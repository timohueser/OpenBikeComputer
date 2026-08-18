//! Typed map failure accounting: incomplete geometry never reaches the painter.

use core::cell::Cell;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_formats::io::Error as IoError;
use obc_formats::obcm::{BRANCH_BIT, EMPTY_LEAF};
use obc_reader::{rgb565_to_rgb888, ByteSource, MapCache, MapTables, Reader};
use obc_render::{RenderConfig, RenderScratch, Viewport, MAX_DECODE_POINTS};
use obcm_testkit::{align_up, build_file, pack_line, pack_line16, pack_line_decl, seal, LodSpec, Style};

mod common;
use common::Buf;

const STYLES: &[Style] = &[(1, 0, 0xF800, 1, 1, false, None)];

fn file(chunk: Vec<u8>, chunk_size: usize) -> Vec<u8> {
    build_file(
        (0, 0, 10_000, 10_000),
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![seal(chunk, chunk_size)], chunk_size }],
    )
}

fn render(reader: &Reader) -> (RenderStatsView, Buf) {
    let vp = Viewport::new(64.0, 64.0, 100, 100, 0.2);
    let mut buf = Buf::new(64, 64);
    let mut renderer = RenderScratch::new();
    let stats = renderer.render(&mut buf, reader, &vp, Rgb888::BLACK, RenderConfig::default(), |color| {
        let (r, g, b) = rgb565_to_rgb888(color);
        Rgb888::new(r, g, b)
    });
    (
        RenderStatsView {
            drawn: stats.features_drawn,
            capacity: stats.feature_decode_capacity_drops,
            malformed: stats.malformed_features,
            structure: stats.map_structure_failures,
            reads: stats.map_read_failures,
        },
        buf,
    )
}

#[derive(Debug, PartialEq, Eq)]
struct RenderStatsView {
    drawn: usize,
    capacity: u32,
    malformed: u32,
    structure: u32,
    reads: u32,
}

#[test]
fn over_capacity_feature_is_dropped_whole_with_typed_stat() {
    let declared = MAX_DECODE_POINTS + 1;
    let deltas = vec![(1i8, 0i8); declared - 1];
    let chunk = pack_line_decl(1, 100, 100, declared as u16, &deltas);
    let bytes = file(chunk.clone(), chunk.len() + 1);
    let src = obc_reader::SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, buf) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 0, capacity: 1, malformed: 0, structure: 0, reads: 0 });
    assert_eq!(buf.count(Rgb888::new(255, 0, 0)), 0, "no prefix of the failed line may be drawn");
}

#[test]
fn failed_high_priority_feature_does_not_block_following_work() {
    let declared = MAX_DECODE_POINTS + 1;
    let mut chunk = pack_line_decl(1, 100, 100, declared as u16, &vec![(1i8, 0i8); declared - 1]);
    chunk.extend_from_slice(&pack_line(2, 100, 120, &[(20, 0)]));
    let chunk_size = chunk.len() + 1;
    let bytes = build_file(
        (0, 0, 10_000, 10_000),
        &[(1, 0, 0xF800, 1, 1, false, None), (2, 1, 0x001F, 2, 4, false, None)],
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![seal(chunk, chunk_size)], chunk_size }],
    );
    let src = obc_reader::SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, buf) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 1, capacity: 1, malformed: 0, structure: 0, reads: 0 });
    assert!(buf.count(Rgb888::new(0, 0, 255)) > 0, "the later complete feature must still draw");
}

#[test]
fn truncated_feature_is_dropped_whole_with_malformed_stat() {
    let chunk = pack_line_decl(1, 100, 100, 40, &[(1, 0); 5]);
    let bytes = file(chunk, 64);
    let src = obc_reader::SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, buf) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 0, capacity: 0, malformed: 1, structure: 0, reads: 0 });
    assert_eq!(buf.count(Rgb888::new(255, 0, 0)), 0, "malformed geometry must not reach the painter");
}

/// Absolute file offset of a LOD's first chunk byte: past the quadtree index, past the v11
/// `chunk_count + 1` entry offset table, and past v14's one rounding step — chunks are addressed by
/// scaled offsets, so they start at `align_up(table_end, U)` and the `0..U-1` bytes in between are
/// §1.2 filler. The failure fixtures below arm on that offset, so they must not confuse the table,
/// or the gap behind it, with the data.
fn chunk_data_offset(lod: &obc_reader::Lod) -> u32 {
    align_up(lod.index_offset + lod.node_count * 4 + (lod.chunk_count + 1) * 4) as u32
}

struct FailAfterParse<'a> {
    bytes: &'a [u8],
    fail_at: Cell<Option<u32>>,
}

impl ByteSource for FailAfterParse<'_> {
    fn read_at(&self, offset: u32, out: &mut [u8]) -> Result<(), IoError> {
        if self.fail_at.get() == Some(offset) {
            return Err(IoError::Io);
        }
        let start = offset as usize;
        let end = start.checked_add(out.len()).ok_or(IoError::BadOffset)?;
        out.copy_from_slice(self.bytes.get(start..end).ok_or(IoError::BadOffset)?);
        Ok(())
    }

    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

#[test]
fn medium_failure_is_distinct_from_decode_failures() {
    let chunk = pack_line_decl(1, 100, 100, 2, &[(1, 0)]);
    let bytes = file(chunk, 64);
    let src = FailAfterParse { bytes: &bytes, fail_at: Cell::new(None) };
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let lod = &reader.lods()[0];
    src.fail_at.set(Some(chunk_data_offset(lod)));

    let (stats, _) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 0, capacity: 0, malformed: 0, structure: 0, reads: 1 });
}

struct FailNthReadAt<'a> {
    bytes: &'a [u8],
    offset: u32,
    fail_on: u8,
    reads: Cell<u8>,
}

impl ByteSource for FailNthReadAt<'_> {
    fn read_at(&self, offset: u32, out: &mut [u8]) -> Result<(), IoError> {
        if offset == self.offset {
            let reads = self.reads.get().saturating_add(1);
            self.reads.set(reads);
            if reads == self.fail_on {
                return Err(IoError::Io);
            }
        }
        let start = offset as usize;
        let end = start.checked_add(out.len()).ok_or(IoError::BadOffset)?;
        out.copy_from_slice(self.bytes.get(start..end).ok_or(IoError::BadOffset)?);
        Ok(())
    }

    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

#[test]
fn unsaturated_cased_feature_needs_no_failure_prone_refetch() {
    // A chunk one byte past the cache-slot size is deliberately uncached. The optimistic collector
    // must still read it only once: fail a hypothetical second read and prove the complete cased
    // feature was already published. (The saturated fallback's transactional pass-B publication is
    // covered by the collector saturation fixtures.)
    //
    // v11 chunks are tight, so the *chunk* has to exceed the slot — a large declared `chunk_size` no
    // longer makes a small chunk uncached. One cased line with 1025 vertices does it (7-byte compact
    // header + 1024 × 4 int16-delta bytes = 4103), and keeping it to a single feature keeps the
    // hypothetical second read unambiguous.
    const CHUNK_SIZE: usize = 8192;
    let zigzag: Vec<(i16, i16)> = (0..1024).map(|i| if i % 2 == 0 { (20, 0) } else { (-20, 0) }).collect();
    let chunk = pack_line16(1, 100, 100, &zigzag);
    assert!(chunk.len() > 4096, "the chunk must outgrow the cache slot to stay uncached: {}", chunk.len());
    let bytes = build_file(
        (0, 0, 10_000, 10_000),
        &[(1, 0, 0xF800, 2, 1, false, Some(0x07E0))],
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![0],
            chunks: vec![seal(chunk, CHUNK_SIZE)],
            chunk_size: CHUNK_SIZE,
        }],
    );
    let layout_src = obc_reader::SliceSource(&bytes);
    let layout = MapTables::parse(&layout_src).unwrap();
    let layout_cache = MapCache::new();
    let layout_reader = Reader::new(&layout_src, &layout, &layout_cache);
    let lod = layout_reader.lods()[0];
    let chunk_offset = chunk_data_offset(&lod);
    let src = FailNthReadAt { bytes: &bytes, offset: chunk_offset, fail_on: 2, reads: Cell::new(0) };
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, buf) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 1, capacity: 0, malformed: 0, structure: 0, reads: 0 });
    assert!(buf.count(Rgb888::new(255, 0, 0)) > 0);
    assert!(buf.count(Rgb888::new(0, 255, 0)) > 0);
}

struct FailSecondIndexWalk<'a> {
    bytes: &'a [u8],
    arm_after: u32,
    fail_block: u32,
    armed: Cell<bool>,
}

impl ByteSource for FailSecondIndexWalk<'_> {
    fn read_at(&self, offset: u32, out: &mut [u8]) -> Result<(), IoError> {
        if self.armed.get() && offset == self.fail_block {
            return Err(IoError::Io);
        }
        let start = offset as usize;
        let end = start.checked_add(out.len()).ok_or(IoError::BadOffset)?;
        out.copy_from_slice(self.bytes.get(start..end).ok_or(IoError::BadOffset)?);
        if offset == self.arm_after {
            // The only geometry chunk is read at the deepest pass-A leaf. The recursive unwind may
            // reload child blocks, but never the root node itself. Arm after serving the chunk; the
            // next source read of the evicted root block is therefore precisely pass B.
            self.armed.set(true);
        }
        Ok(())
    }

    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

fn sparse_index_chain(levels: usize) -> Vec<u32> {
    const WORDS_PER_BLOCK: usize = 512 / 4;
    let mut index = vec![EMPTY_LEAF; levels * WORDS_PER_BLOCK + 4];
    index[0] = BRANCH_BIT | WORDS_PER_BLOCK as u32;
    for level in 1..levels {
        // Follow the SW child (`base + 2`), which keeps intersecting render()'s low-coordinate
        // viewport while the other three siblings remain empty.
        let idx = level * WORDS_PER_BLOCK + 2;
        index[idx] = BRANCH_BIT | ((level + 1) * WORDS_PER_BLOCK) as u32;
    }
    index[levels * WORDS_PER_BLOCK + 2] = 0;
    index
}

#[test]
fn resident_winner_skips_the_second_index_walk() {
    const LEVELS: usize = 24; // beyond the seven-block index cache
    let bytes = build_file(
        (0, 0, 10_000, 10_000),
        STYLES,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: sparse_index_chain(LEVELS),
            chunks: vec![seal(pack_line(1, 100, 100, &[(20, 0)]), 64)],
            chunk_size: 64,
        }],
    );
    let layout_src = obc_reader::SliceSource(&bytes);
    let layout = MapTables::parse(&layout_src).unwrap();
    let layout_cache = MapCache::new();
    let layout_reader = Reader::new(&layout_src, &layout, &layout_cache);
    let lod = layout_reader.lods()[0];
    let index_offset = lod.index_offset as u32;
    let chunk_offset = chunk_data_offset(&lod);
    let block = |offset: u32| offset - offset % 512;
    let src = FailSecondIndexWalk {
        bytes: &bytes,
        arm_after: chunk_offset,
        fail_block: block(index_offset),
        armed: Cell::new(false),
    };
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, buf) = render(&reader);
    assert!(src.armed.get(), "fixture must arm only after pass A completes");
    assert_eq!(stats, RenderStatsView { drawn: 1, capacity: 0, malformed: 0, structure: 0, reads: 0 });
    assert!(buf.count(Rgb888::new(255, 0, 0)) > 0, "the resident pass-A chunk must decode without another walk");
}

#[test]
fn cached_leaf_list_skips_second_index_walk_for_uncached_geometry() {
    const LEVELS: usize = 24; // beyond the seven-block index cache
    const CHUNK_SIZE: usize = 8192;
    // Make the sole chunk larger than the four-KiB geometry slots. Pass A can still select the
    // first tiny line, but pass B has no resident geometry and must read the chunk again. The leaf
    // list should still avoid a second index walk; the armed source turns any such walk into a
    // failure, so both features drawing proves the index stayed quiet.
    let mut chunk = pack_line(1, 100, 100, &[(20, 0)]);
    chunk.extend_from_slice(&pack_line16(1, 100, 100, &vec![(1, 0); 1024]));
    assert!(chunk.len() > 4096);
    let bytes = build_file(
        (0, 0, 10_000, 10_000),
        STYLES,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: sparse_index_chain(LEVELS),
            chunks: vec![seal(chunk, CHUNK_SIZE)],
            chunk_size: CHUNK_SIZE,
        }],
    );
    let layout_src = obc_reader::SliceSource(&bytes);
    let layout = MapTables::parse(&layout_src).unwrap();
    let layout_cache = MapCache::new();
    let layout_reader = Reader::new(&layout_src, &layout, &layout_cache);
    let lod = layout_reader.lods()[0];
    let index_offset = lod.index_offset as u32;
    let chunk_offset = chunk_data_offset(&lod);
    let block = |offset: u32| offset - offset % 512;
    let src = FailSecondIndexWalk {
        bytes: &bytes,
        arm_after: chunk_offset,
        fail_block: block(index_offset),
        armed: Cell::new(false),
    };
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, buf) = render(&reader);
    assert!(src.armed.get(), "fixture must arm only after pass A completes");
    assert_eq!(stats, RenderStatsView { drawn: 2, capacity: 0, malformed: 0, structure: 0, reads: 0 });
    assert!(buf.count(Rgb888::new(255, 0, 0)) > 0);
}

#[test]
fn corrupt_chunk_reference_has_its_own_structure_stat() {
    let bytes = build_file(
        (0, 0, 10_000, 10_000),
        STYLES,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![1], // only chunk 0 exists
            chunks: vec![seal(pack_line(1, 100, 100, &[(20, 0)]), 64)],
            chunk_size: 64,
        }],
    );
    let src = obc_reader::SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, _) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 0, capacity: 0, malformed: 0, structure: 1, reads: 0 });
}
