//! Bounded photo work over a selected map window. The host owns the pixel target.

use miniz_oxide::inflate::{
    core::{decompress, inflate_flags::*, DecompressorOxide},
    TINFLStatus,
};
use obc_formats::{
    io::{ByteSource, Error as SourceError},
    obcm::landmarks::{PHOTO_HISTORY, PHOTO_MAX_COMPRESSED, PHOTO_PIXELS, PHOTO_WINDOW_BITS},
};

pub const INPUT_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Source(SourceError),
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Pending,
    Complete,
}

/// Retain in the host's scratch arena, not the screen or a second framebuffer.
/// Each step reads at most 256 bytes and emits at most 4096 completed pixels.
/// The caller binds this state to one immutable source and clears its rectangle
/// on cancellation or failure. No source or pixel borrow survives a step.
pub struct PhotoDecoder {
    decoder: DecompressorOxide,
    history: [u8; PHOTO_HISTORY],
    read: usize,
    written: usize,
    result: Result<Progress, Error>,
}

impl Default for PhotoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl PhotoDecoder {
    pub fn new() -> Self {
        Self {
            decoder: DecompressorOxide::new(),
            history: [0; PHOTO_HISTORY],
            read: 0,
            written: 0,
            result: Ok(Progress::Pending),
        }
    }

    pub fn reset(&mut self) {
        self.decoder.init();
        self.read = 0;
        self.written = 0;
        self.result = Ok(Progress::Pending);
    }

    pub fn pixels_written(&self) -> usize {
        self.written
    }

    pub fn step(&mut self, source: &dyn ByteSource, mut pixels: impl FnMut(usize, &[u8])) -> Result<Progress, Error> {
        if self.result != Ok(Progress::Pending) {
            return self.result;
        }
        self.result = self.decode_step(source, &mut pixels);
        self.result
    }

    fn decode_step(
        &mut self,
        source: &dyn ByteSource,
        pixels: &mut impl FnMut(usize, &[u8]),
    ) -> Result<Progress, Error> {
        let len = source.len();
        if !(6..=PHOTO_MAX_COMPRESSED as u64).contains(&len) {
            return Err(Error::Invalid);
        }
        let len = len as usize;
        let end = self.read.saturating_add(INPUT_BYTES).min(len);
        let mut input = [0; INPUT_BYTES];
        let input = input.get_mut(..end.checked_sub(self.read).ok_or(Error::Invalid)?).ok_or(Error::Invalid)?;
        if !input.is_empty() {
            source.read_at(self.read as u64, input).map_err(Error::Source)?;
        }
        if self.read == 0 && (input.len() < 2 || input[0] & 15 != 8 || input[0] >> 4 > PHOTO_WINDOW_BITS - 8) {
            return Err(Error::Invalid);
        }
        let mut flags = TINFL_FLAG_PARSE_ZLIB_HEADER;
        // Before the first wrap, reject distances into unproduced history.
        if self.written < PHOTO_HISTORY {
            flags |= TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
        }
        if end < len {
            flags |= TINFL_FLAG_HAS_MORE_INPUT;
        }
        let position = self.written % PHOTO_HISTORY;
        let (status, consumed, produced) = decompress(&mut self.decoder, input, &mut self.history, position, flags);
        let total = self.written.checked_add(produced).ok_or(Error::Invalid)?;
        let output = self.history.get(position..position + produced).ok_or(Error::Invalid)?;
        if total > PHOTO_PIXELS || output.iter().any(|&pixel| pixel >= 64) {
            return Err(Error::Invalid);
        }
        self.read += consumed;
        let progress = match status {
            TINFLStatus::Done if self.read == len && total == PHOTO_PIXELS => Progress::Complete,
            TINFLStatus::NeedsMoreInput | TINFLStatus::HasMoreOutput if consumed + produced > 0 => Progress::Pending,
            _ => return Err(Error::Invalid),
        };
        if !output.is_empty() {
            pixels(self.written, output);
        }
        self.written = total;
        Ok(progress)
    }
}
