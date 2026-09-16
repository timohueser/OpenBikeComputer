use obc_formats::{
    io::{ByteSource, Error as SourceError, SliceSource, WindowSource},
    obcm::landmarks::{PHOTO_HISTORY, PHOTO_MAX_COMPRESSED, PHOTO_PIXELS},
};
use obc_reader::photo::{Error, PhotoDecoder, Progress, INPUT_BYTES};
use std::cell::Cell;

fn compress(pixels: &[u8], window_bits: i32) -> Vec<u8> {
    let mut bytes = vec![0; zlib_rs::compress_bound(pixels.len())];
    let (output, code) = zlib_rs::compress_slice(
        &mut bytes,
        pixels,
        zlib_rs::DeflateConfig { level: 9, window_bits, ..Default::default() },
    );
    assert_eq!(code, zlib_rs::ReturnCode::Ok);
    output.to_vec()
}

struct CountingSource<'a> {
    bytes: &'a [u8],
    reads: Cell<usize>,
    fail_at: Option<u64>,
}

impl ByteSource for CountingSource<'_> {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
        assert!(output.len() <= INPUT_BYTES);
        self.reads.set(self.reads.get() + 1);
        if self.fail_at.is_some_and(|at| offset >= at) {
            return Err(SourceError::Io);
        }
        SliceSource(self.bytes).read_at(offset, output)
    }
}

fn decode(decoder: &mut PhotoDecoder, source: &dyn ByteSource) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    for _ in 0..1024 {
        let progress = decoder.step(source, |offset, pixels| {
            assert_eq!(offset, output.len());
            assert!(pixels.len() <= PHOTO_HISTORY);
            output.extend_from_slice(pixels);
        })?;
        if progress == Progress::Complete {
            assert_eq!(decoder.pixels_written(), PHOTO_PIXELS);
            return Ok(output);
        }
    }
    panic!("decoder did not terminate within its input/output work bound")
}

#[test]
fn independently_seekable_photos_roundtrip_with_bounded_reads_and_reset() {
    let pictures = [vec![63; PHOTO_PIXELS], (0..PHOTO_PIXELS).map(|i| ((i * 31 + i / 23) % 64) as u8).collect()];
    let streams = pictures.each_ref().map(|p| compress(p, 12));
    let mut map = vec![0; 17];
    map.extend_from_slice(&streams[0]);
    map.extend_from_slice(&streams[1]);
    let source = CountingSource { bytes: &map, reads: Cell::new(0), fail_at: None };
    let mut decoder = PhotoDecoder::new();
    for i in [1, 0] {
        decoder.reset();
        let start = 17 + if i == 1 { streams[0].len() } else { 0 };
        let window = WindowSource::new(&source, start as u64, streams[i].len() as u64).unwrap();
        assert_eq!(decode(&mut decoder, &window).unwrap(), pictures[i]);
        let before = source.reads.get();
        assert_eq!(decoder.step(&window, |_, _| panic!("completed stream wrote twice")), Ok(Progress::Complete));
        assert_eq!(source.reads.get(), before);
    }
}

#[test]
fn malformed_streams_never_complete_and_errors_are_sticky() {
    let good = compress(&vec![25; PHOTO_PIXELS], 12);
    let mut bad_checksum = good.clone();
    *bad_checksum.last_mut().unwrap() ^= 1;
    let mut trailing = good.clone();
    trailing.push(0);
    let invalid = [
        Vec::new(),
        vec![0; PHOTO_MAX_COMPRESSED + 1],
        good[..good.len() - 1].to_vec(),
        bad_checksum,
        trailing,
        compress(&vec![25; PHOTO_PIXELS - 1], 12),
        compress(&vec![25; PHOTO_PIXELS + 1], 12),
        compress(&vec![64; PHOTO_PIXELS], 12),
        compress(&vec![25; PHOTO_PIXELS], 15),
    ];
    for bytes in invalid {
        let source = CountingSource { bytes: &bytes, reads: Cell::new(0), fail_at: None };
        let mut decoder = PhotoDecoder::new();
        assert_eq!(decode(&mut decoder, &source), Err(Error::Invalid));
        let before = source.reads.get();
        assert_eq!(decoder.step(&source, |_, _| panic!("failed stream wrote pixels")), Err(Error::Invalid));
        assert_eq!(source.reads.get(), before);
    }
}

#[test]
fn medium_failure_remains_an_error_and_reset_reconstructs_pixels() {
    let pixels: Vec<_> = (0..PHOTO_PIXELS).map(|i| ((i * 31 + i / 23) % 64) as u8).collect();
    let bytes = compress(&pixels, 12);
    let source = CountingSource { bytes: &bytes, reads: Cell::new(0), fail_at: Some(INPUT_BYTES as u64) };
    let mut decoder = PhotoDecoder::new();
    assert_eq!(decode(&mut decoder, &source), Err(Error::Source(SourceError::Io)));
    assert!(decoder.pixels_written() > 0);
    decoder.reset();
    assert_eq!(decode(&mut decoder, &SliceSource(&bytes)).unwrap(), pixels);
}

#[test]
fn each_step_performs_at_most_one_source_read() {
    let bytes = compress(&vec![0; PHOTO_PIXELS], 12);
    let source = CountingSource { bytes: &bytes, reads: Cell::new(0), fail_at: None };
    let mut decoder = PhotoDecoder::new();
    loop {
        let before = source.reads.get();
        let progress = decoder.step(&source, |_, pixels| assert!(pixels.len() <= PHOTO_HISTORY)).unwrap();
        assert!(source.reads.get() - before <= 1);
        if progress == Progress::Complete {
            break;
        }
    }
}
