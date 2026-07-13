//! Typed map failure accounting: incomplete geometry never reaches the painter.

use core::cell::Cell;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_reader::byte_io::Error as IoError;
use obc_reader::{rgb565_to_rgb888, ByteSource, MapCache, MapTables, Reader};
use obc_render::{MapRenderer, Viewport, MAX_DECODE_POINTS};
use obcm_testkit::{build_file, pack_line, pack_line_decl, pad, LodSpec, Style};

mod common;
use common::Buf;

const STYLES: &[Style] = &[(1, 0, 0xF800, 1, 1, false, None)];

fn file(chunk: Vec<u8>, chunk_size: usize) -> Vec<u8> {
    build_file(
        (0, 0, 10_000, 10_000),
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![pad(chunk, chunk_size)], chunk_size }],
    )
}

fn render(reader: &Reader) -> (RenderStatsView, Buf) {
    let vp = Viewport::new(64.0, 64.0, 100, 100, 0.2);
    let mut buf = Buf::new(64, 64);
    let mut renderer = MapRenderer::new();
    let stats = renderer.render(&mut buf, reader, &vp, Rgb888::BLACK, |color| {
        let (r, g, b) = rgb565_to_rgb888(color);
        Rgb888::new(r, g, b)
    });
    (
        RenderStatsView {
            drawn: stats.features_drawn,
            capacity: stats.feature_decode_capacity_drops,
            malformed: stats.malformed_features,
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
    assert_eq!(stats, RenderStatsView { drawn: 0, capacity: 1, malformed: 0, reads: 0 });
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
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![pad(chunk, chunk_size)], chunk_size }],
    );
    let src = obc_reader::SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);

    let (stats, buf) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 1, capacity: 1, malformed: 0, reads: 0 });
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
    assert_eq!(stats, RenderStatsView { drawn: 0, capacity: 0, malformed: 1, reads: 0 });
    assert_eq!(buf.count(Rgb888::new(255, 0, 0)), 0, "malformed geometry must not reach the painter");
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
    src.fail_at.set(Some((lod.index_offset + lod.node_count * 4) as u32));

    let (stats, _) = render(&reader);
    assert_eq!(stats, RenderStatsView { drawn: 0, capacity: 0, malformed: 0, reads: 1 });
}
