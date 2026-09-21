//! Merge landmark identities and shared content without retaining source photos in memory.

use std::collections::BTreeMap;

use obc_formats::{
    io::rd_u32,
    obcm::{landmarks::*, HEADER_LANDMARK_OFFSET_OFF, POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE},
};
use obc_reader::landmarks::{map_section, LandmarkDirectory};
use sha2::{Digest, Sha256};

use crate::{emit::MapWriter, input::Cell, poi::MergedPois, Error, Result};

const COPY_BYTES: usize = 4096;
type Schedule = [u8; POI_HOURS_BLOB_LEN];

#[derive(Clone)]
pub(crate) struct Blob {
    pub(crate) cell: usize,
    pub(crate) offset: u64,
    pub(crate) len: u32,
    pub(crate) hash: [u8; 32],
}

impl Blob {
    pub(crate) fn copy(&self, cells: &[&Cell<'_>], mut sink: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let mut buffer = [0; COPY_BYTES];
        let mut done = 0;
        while done < self.len {
            let count = (self.len - done).min(COPY_BYTES as u32) as usize;
            cells[self.cell].read_into(self.offset + u64::from(done), &mut buffer[..count])?;
            sink(&buffer[..count])?;
            done += count as u32;
        }
        Ok(())
    }

    pub(crate) fn same_bytes(&self, other: &Self, cells: &[&Cell<'_>]) -> Result<bool> {
        if self.len != other.len || self.hash != other.hash {
            return Ok(false);
        }
        let mut buffer = [0; COPY_BYTES];
        let mut done = 0;
        let mut equal = true;
        self.copy(cells, |bytes| {
            cells[other.cell].read_into(other.offset + done, &mut buffer[..bytes.len()])?;
            equal &= bytes == &buffer[..bytes.len()];
            done += bytes.len() as u64;
            Ok(())
        })?;
        Ok(equal)
    }
}

struct Candidate {
    record: LandmarkRecord,
    blobs: [Option<Blob>; 4],
    schedule: Option<Schedule>,
    hash: [u8; 32],
}

impl Candidate {
    fn precedence(&self) -> (bool, u64, [u8; 32]) {
        (
            self.record.osm.is_none_or(|osm| osm.approach.is_none()),
            self.record.osm.map_or(u64::MAX, |osm| osm.source.0),
            self.hash,
        )
    }
}

/// Output records and source-backed content. Memory is bounded by the format's record limit.
#[derive(Default)]
pub struct LandmarkSection {
    records: Vec<LandmarkRecord>,
    blobs: Vec<Blob>,
    len: u32,
}

impl LandmarkSection {
    pub fn section_len(&self) -> u64 {
        if self.records.is_empty() {
            0
        } else {
            crate::emit::align_up(u64::from(self.len))
        }
    }

    pub fn emit(&self, cells: &[&Cell<'_>], out: &mut MapWriter<'_>) -> Result<()> {
        if self.records.is_empty() {
            return Ok(());
        }
        let mut header = [0; SECTION_HEADER_LEN];
        header[..4].copy_from_slice(&(self.records.len() as u32).to_le_bytes());
        header[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
        header[6..8].copy_from_slice(&SECTION_VERSION.to_le_bytes());
        header[8..12].copy_from_slice(
            &(SECTION_HEADER_LEN as u32 + self.records.len() as u32 * RECORD_LEN as u32).to_le_bytes(),
        );
        header[12..16].copy_from_slice(&self.len.to_le_bytes());
        out.put(&header)?;
        for record in &self.records {
            out.put(&record.encode())?;
        }
        for blob in &self.blobs {
            let mut hash = Sha256::new();
            blob.copy(cells, |bytes| {
                hash.update(bytes);
                out.put(bytes)
            })?;
            if <[u8; 32]>::from(hash.finalize()) != blob.hash {
                return Err(Error::Verify("landmark source content changed during assembly".into()));
            }
        }
        out.begin_section()?;
        Ok(())
    }
}

pub(crate) fn malformed(error: obc_reader::Error) -> Error {
    match error {
        obc_reader::Error::Source(error) => Error::Io(error),
        other => Error::Format(format!("invalid landmark section: {other:?}")),
    }
}

fn references(record: &LandmarkRecord) -> [ContentRef; 4] {
    [record.name, record.articles, record.photo, record.photo_attribution]
}

fn set_references(record: &mut LandmarkRecord, refs: [ContentRef; 4]) {
    [record.name, record.articles, record.photo, record.photo_attribution] = refs;
}

/// Resolve file-local schedules before records or pools are laid out. Content precedence is
/// independent of cell order.
pub fn merge(cells: &[&Cell<'_>], pois: &mut MergedPois) -> Result<LandmarkSection> {
    let mut winners = BTreeMap::<u64, Candidate>::new();
    for (cell_index, cell) in cells.iter().enumerate() {
        let Some(section) = map_section(cell.src).map_err(malformed)? else { continue };
        let directory = LandmarkDirectory::read(&section).map_err(malformed)?;
        let mut offset = [0; 4];
        cell.read_into(HEADER_LANDMARK_OFFSET_OFF as u64, &mut offset)?;
        let start = crate::emit::SCALE.offset(rd_u32(&offset, 0)).bytes();
        let pool = crate::poi::read_hours_pool(cell)?;
        let mut last = None;
        for index in 0..directory.count {
            let record = directory.record(&section, index).map_err(malformed)?;
            directory.article(&section, &record, *b"en").map_err(malformed)?;
            if last.is_some_and(|key| key >= record.key()) {
                return Err(Error::Format("landmark latitude index is not strictly ordered".into()));
            }
            last = Some(record.key());
            if record.photo.is_absent() != record.photo_attribution.is_absent() {
                return Err(Error::Format("landmark photo and attribution must be present together".into()));
            }
            let schedule = if record.hours_ref == POI_HOURS_REF_NONE {
                None
            } else {
                let blob = *pool
                    .get(record.hours_ref as usize)
                    .ok_or_else(|| Error::Format("landmark hours reference is past its cell pool".into()))?;
                if obc_reader::WeeklySchedule::decode(&blob).is_none() {
                    return Err(Error::Format("invalid landmark weekly schedule".into()));
                }
                Some(blob)
            };
            let mut blobs: [Option<Blob>; 4] = std::array::from_fn(|_| None);
            let limits =
                [MAX_NAME_BYTES, obc_formats::articles::MAX_BYTES, PHOTO_MAX_COMPRESSED as u32, MAX_ATTRIBUTION_BYTES];
            for (i, reference) in references(&record).into_iter().enumerate() {
                if i >= 2 && reference.is_absent() {
                    continue;
                }
                let range = reference
                    .range(directory.payload, directory.len, limits[i])
                    .ok_or_else(|| Error::Format("landmark content reference is out of bounds".into()))?;
                let mut blob =
                    Blob { cell: cell_index, offset: start + range.start, len: reference.len, hash: [0; 32] };
                let mut hash = Sha256::new();
                blob.copy(cells, |bytes| {
                    hash.update(bytes);
                    Ok(())
                })?;
                blob.hash = hash.finalize().into();
                blobs[i] = Some(blob);
            }
            // Exclude file-local positions and schedule indexes from canonical content precedence.
            let mut canonical = record;
            canonical.hours_ref = POI_HOURS_REF_NONE;
            set_references(
                &mut canonical,
                references(&record).map(|reference| ContentRef { offset: 0, len: reference.len }),
            );
            let mut hash = Sha256::new();
            hash.update(canonical.encode());
            if let Some(schedule) = schedule {
                hash.update(schedule);
            }
            for blob in blobs.iter().flatten() {
                hash.update(blob.hash);
            }
            let candidate = Candidate { record, blobs, schedule, hash: hash.finalize().into() };
            match winners.entry(record.qid) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(candidate);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if candidate.precedence() < entry.get().precedence() {
                        entry.insert(candidate);
                    }
                }
            }
            if winners.len() > MAX_RECORDS as usize {
                return Err(Error::Capacity("assembled landmark count exceeds the format limit".into()));
            }
        }
    }
    pois.add_hours(winners.values().filter_map(|candidate| candidate.schedule))?;
    let mut winners: Vec<_> = winners.into_values().collect();
    winners.sort_unstable_by_key(|candidate| candidate.record.key());
    let mut cursor = SECTION_HEADER_LEN as u32 + winners.len() as u32 * RECORD_LEN as u32;
    let mut records = Vec::with_capacity(winners.len());
    let mut blobs: Vec<Blob> = Vec::new();
    let mut interned = BTreeMap::<[u8; 32], Vec<(usize, ContentRef)>>::new();
    for mut candidate in winners {
        candidate.record.hours_ref = candidate.schedule.map_or(POI_HOURS_REF_NONE, |blob| {
            pois.pool.binary_search(&blob).expect("landmark schedule joined the shared pool") as u16
        });
        let mut refs = [ContentRef::default(); 4];
        for (i, blob) in candidate.blobs.into_iter().enumerate() {
            let Some(blob) = blob else { continue };
            let bucket = interned.entry(blob.hash).or_default();
            let mut existing = None;
            for &(index, reference) in bucket.iter() {
                if blob.same_bytes(&blobs[index], cells)? {
                    existing = Some(reference);
                    break;
                }
            }
            refs[i] = if let Some(reference) = existing {
                reference
            } else {
                let reference = ContentRef { offset: cursor, len: blob.len };
                cursor = cursor
                    .checked_add(blob.len)
                    .ok_or_else(|| Error::Capacity("landmark content exceeds u32 section offsets".into()))?;
                bucket.push((blobs.len(), reference));
                blobs.push(blob);
                reference
            };
        }
        set_references(&mut candidate.record, refs);
        records.push(candidate.record);
    }
    Ok(LandmarkSection { records, blobs, len: cursor })
}

#[cfg(test)]
#[path = "../tests/support/landmarks.rs"]
mod fixture;

#[cfg(test)]
mod tests {
    use super::fixture;
    use super::*;
    use crate::{grid::CellId, input::CellInput};
    use obc_formats::{
        io::{ByteSource, SliceSource},
        obcm::{PoiApproach, PoiMetadata, SourceId},
    };

    const WEST: CellId = CellId { log2: 18, i: 1204, j: 1052 };
    const EAST: CellId = CellId { log2: 18, i: 1204, j: 1053 };

    fn source(id: CellId, records: Vec<LandmarkRecord>, hours: &[Schedule]) -> Vec<u8> {
        let (w, s, e, n) = id.square();
        let mut map = obcm_testkit::build_poi_map_with_hours((w as i32, s as i32, e as i32, n as i32), 512, &[], hours);
        fixture::attach(&mut map, records, true);
        map
    }

    fn metadata(id: u64, mapped: bool) -> Option<PoiMetadata> {
        Some(PoiMetadata {
            source: SourceId::osm(1, id),
            approach: mapped.then_some(PoiApproach {
                source: SourceId::osm(1, 99),
                lat: 47_300_000,
                lon: 7_500_000,
                profile_mask: 1,
            }),
        })
    }

    fn run(sources: &[(CellId, Vec<u8>)]) -> (LandmarkSection, MergedPois, Vec<u8>) {
        let cache = obc_reader::MapCache::new_boxed();
        let srcs: Vec<_> = sources.iter().map(|(_, bytes)| SliceSource(bytes)).collect();
        let cells: Vec<_> = sources
            .iter()
            .zip(&srcs)
            .map(|((id, _), src)| {
                Cell::open(CellInput { id: *id, band: "network".into(), src, partial: false }, &cache).unwrap()
            })
            .collect();
        let cells: Vec<_> = cells.iter().collect();
        let mut pois = crate::poi::merge(&cells).unwrap();
        let section = merge(&cells, &mut pois).unwrap();
        let mut bytes = Vec::new();
        section
            .emit(
                &cells,
                &mut MapWriter::new(crate::emit::SCALE, 0, &mut |data| {
                    bytes.extend_from_slice(data);
                    Ok(())
                }),
            )
            .unwrap();
        (section, pois, bytes)
    }

    #[test]
    fn adjacent_cells_remap_colliding_hours_and_intern_content() {
        let mut a = fixture::record(1);
        a.osm = metadata(1, false);
        a.hours_ref = 0;
        let mut b = fixture::record(2);
        b.osm = metadata(2, false);
        b.hours_ref = 0;
        let closed = [0; POI_HOURS_BLOB_LEN];
        let mut open = closed;
        open[2] = 96;
        let inputs = [(WEST, source(WEST, vec![a], &[open])), (EAST, source(EAST, vec![b], &[closed]))];
        let (section, pois, bytes) = run(&inputs);
        assert_eq!(pois.pool, vec![closed, open]);
        assert_eq!(section.records[0].hours_ref, 1);
        assert_eq!(section.records[1].hours_ref, 0);
        assert_eq!(section.records[0].photo, section.records[1].photo);
        assert_eq!(section.records[0].articles, section.records[1].articles);
        assert_eq!(section.blobs.len(), 4);
        let src = SliceSource(&bytes);
        let directory = LandmarkDirectory::read(&src).unwrap();
        fixture::assert_content(&src);
        assert_eq!(directory.count, 2);
        assert_eq!(directory.record(&src, 0).unwrap(), section.records[0]);
        let mut shuffled = inputs;
        shuffled.reverse();
        assert_eq!(run(&shuffled).2, bytes);
    }

    #[test]
    fn duplicate_precedence_is_mapped_then_identity_then_content() {
        for (first_mapped, second_mapped, first_id, second_id, expected) in
            [(false, true, 1, 2, 2), (true, true, 3, 2, 2)]
        {
            let mut a = fixture::record(1);
            a.osm = metadata(first_id, first_mapped);
            let mut b = a;
            b.osm = metadata(second_id, second_mapped);
            let mut inputs = [(WEST, source(WEST, vec![a], &[])), (EAST, source(EAST, vec![b], &[]))];
            let (section, _, bytes) = run(&inputs);
            assert_eq!(section.records.len(), 1);
            assert_eq!(section.records[0].osm.unwrap().source, SourceId::osm(1, expected));
            inputs.reverse();
            assert_eq!(run(&inputs).2, bytes);
        }
        let a = fixture::record(1);
        let mut b = a;
        b.category = 2;
        let mut inputs = [(WEST, source(WEST, vec![a], &[])), (EAST, source(EAST, vec![b], &[]))];
        let (_, _, bytes) = run(&inputs);
        inputs.reverse();
        assert_eq!(run(&inputs).2, bytes);
    }

    #[test]
    fn invalid_references_fail_even_in_a_losing_duplicate() {
        for field in [52, 60, 68, 76, 20] {
            let mut bad = fixture::record(1);
            bad.osm = metadata(2, false);
            let mut bytes = source(EAST, vec![bad], &[]);
            let start = rd_u32(&bytes, HEADER_LANDMARK_OFFSET_OFF) as usize * 16 + SECTION_HEADER_LEN;
            bytes[start + field..start + field + 2].copy_from_slice(&0xfffeu16.to_le_bytes());
            let cache = obc_reader::MapCache::new_boxed();
            let src = SliceSource(&bytes);
            let cell =
                Cell::open(CellInput { id: EAST, band: "network".into(), src: &src, partial: false }, &cache).unwrap();
            let mut good = fixture::record(1);
            good.osm = metadata(1, true);
            let good = source(WEST, vec![good], &[]);
            let good = SliceSource(&good);
            let good =
                Cell::open(CellInput { id: WEST, band: "network".into(), src: &good, partial: false }, &cache).unwrap();
            let mut pois = crate::poi::merge(&[&good, &cell]).unwrap();
            assert!(merge(&[&good, &cell], &mut pois).is_err(), "field {field}");
        }
    }

    #[test]
    fn source_changes_and_io_failures_abort_emission() {
        use std::cell::{Cell as Flag, RefCell};
        struct Mutable {
            bytes: RefCell<Vec<u8>>,
            fail: Flag<bool>,
        }
        impl ByteSource for Mutable {
            fn len(&self) -> u64 {
                self.bytes.borrow().len() as u64
            }
            fn read_at(&self, offset: u64, out: &mut [u8]) -> std::result::Result<(), obc_formats::io::Error> {
                if self.fail.get() {
                    return Err(obc_formats::io::Error::Io);
                }
                SliceSource(&self.bytes.borrow()).read_at(offset, out)
            }
        }
        let src = Mutable { bytes: RefCell::new(source(WEST, vec![fixture::record(1)], &[])), fail: Flag::new(false) };
        let cache = obc_reader::MapCache::new_boxed();
        let cell =
            Cell::open(CellInput { id: WEST, band: "network".into(), src: &src, partial: false }, &cache).unwrap();
        let mut pois = crate::poi::merge(&[&cell]).unwrap();
        let section = merge(&[&cell], &mut pois).unwrap();
        src.bytes.borrow_mut()[section.blobs[0].offset as usize] ^= 1;
        let emit = || section.emit(&[&cell], &mut MapWriter::new(crate::emit::SCALE, 0, &mut |_| Ok(())));
        assert!(matches!(emit(), Err(Error::Verify(_))));
        src.fail.set(true);
        assert!(matches!(emit(), Err(Error::Io(obc_formats::io::Error::Io))));
    }

    #[test]
    fn optional_empty_sections_and_bounded_photo_reads() {
        let (_, _, bytes) = run(&[(WEST, source(WEST, vec![], &[]))]);
        assert!(bytes.is_empty());
        struct Bounded(Vec<u8>);
        impl ByteSource for Bounded {
            fn len(&self) -> u64 {
                self.0.len() as u64
            }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::result::Result<(), obc_formats::io::Error> {
                assert!(buf.len() <= COPY_BYTES);
                SliceSource(&self.0).read_at(offset, buf)
            }
        }
        let src = Bounded(source(WEST, vec![fixture::record(1)], &[]));
        let cache = obc_reader::MapCache::new_boxed();
        let cell =
            Cell::open(CellInput { id: WEST, band: "network".into(), src: &src, partial: false }, &cache).unwrap();
        let mut pois = crate::poi::merge(&[&cell]).unwrap();
        let section = merge(&[&cell], &mut pois).unwrap();
        section.emit(&[&cell], &mut MapWriter::new(crate::emit::SCALE, 0, &mut |_| Ok(()))).unwrap();
        let mut absent = src.0;
        absent[49..57].fill(0);
        assert!(run(&[(WEST, absent)]).2.is_empty());
    }
}
