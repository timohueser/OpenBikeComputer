//! A block device that records which sectors were written — the instrument §13.1's clean-flush
//! obligation is measured with.
//!
//! The clean-flush rule is not a property any test of file *contents* can observe: a store whose
//! flush rewrites FSInfo and the directory entry on every sync produces exactly the same bytes as
//! one that does not, right up to the power cut that destroys a single-copy sector. The only way to
//! know is to watch the LBAs. This wrapper sits between the FAT layer and the real card, counts
//! every read and write, and records the write spans while recording is armed.
//!
//! It is `no_std` and allocation-free, so the same instrument runs in a host test against a
//! synthetic volume and on the board against the real card. Recording is off until armed and the
//! entry buffer is bounded: an overrun is counted rather than dropped silently, because "the log
//! filled up" and "nothing else was written" are opposite conclusions.

use core::cell::RefCell;

use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};

/// One recorded write span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    /// The first LBA of the write.
    pub start: u32,
    /// How many 512-byte blocks it covered.
    pub blocks: u32,
}

impl Span {
    /// Whether `lba` lies inside this span.
    pub fn contains(&self, lba: u32) -> bool {
        lba >= self.start && lba - self.start < self.blocks
    }
}

/// What the wrapper counted, independent of whether recording was armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    /// `BlockDevice::read` calls.
    pub reads: u32,
    /// Blocks those reads covered.
    pub read_blocks: u32,
    /// `BlockDevice::write` calls.
    pub writes: u32,
    /// Blocks those writes covered.
    pub written_blocks: u32,
}

struct State<const N: usize> {
    counters: Counters,
    spans: heapless::Vec<Span, N>,
    dropped: u32,
    recording: bool,
}

/// A [`BlockDevice`] that counts every operation and records write spans while armed.
pub struct WriteLog<D, const N: usize = 64> {
    device: D,
    state: RefCell<State<N>>,
}

impl<D: BlockDevice, const N: usize> WriteLog<D, N> {
    /// Wraps a block device. Recording starts disarmed; the counters start at zero.
    pub fn new(device: D) -> Self {
        WriteLog {
            device,
            state: RefCell::new(State {
                counters: Counters::default(),
                spans: heapless::Vec::new(),
                dropped: 0,
                recording: false,
            }),
        }
    }

    /// Clears the counters and the span log and arms recording.
    pub fn arm(&self) {
        let mut state = self.state.borrow_mut();
        state.counters = Counters::default();
        state.spans.clear();
        state.dropped = 0;
        state.recording = true;
    }

    /// Disarms recording, leaving the counters and spans readable.
    pub fn disarm(&self) {
        self.state.borrow_mut().recording = false;
    }

    /// The counters since the last [`arm`](Self::arm).
    pub fn counters(&self) -> Counters {
        self.state.borrow().counters
    }

    /// How many spans did not fit the bounded log.
    pub fn dropped(&self) -> u32 {
        self.state.borrow().dropped
    }

    /// Reads the recorded spans. A closure rather than a slice, because the log lives behind a
    /// `RefCell` and handing out a borrow across a device call would panic at the next write.
    pub fn with_spans<R>(&self, f: impl FnOnce(&[Span]) -> R) -> R {
        f(&self.state.borrow().spans)
    }

    /// How many recorded spans touched `lba`.
    pub fn writes_touching(&self, lba: u32) -> usize {
        self.with_spans(|spans| spans.iter().filter(|span| span.contains(lba)).count())
    }

    /// The device underneath, for the raw sector reads a geometry probe needs.
    pub fn device(&self) -> &D {
        &self.device
    }
}

impl<D: BlockDevice, const N: usize> BlockDevice for WriteLog<D, N> {
    type Error = D::Error;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), Self::Error> {
        {
            let mut state = self.state.borrow_mut();
            state.counters.reads += 1;
            state.counters.read_blocks += blocks.len() as u32;
        }
        self.device.read(blocks, start)
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), Self::Error> {
        {
            let mut state = self.state.borrow_mut();
            state.counters.writes += 1;
            state.counters.written_blocks += blocks.len() as u32;
            if state.recording {
                let span = Span { start: start.0, blocks: blocks.len() as u32 };
                if state.spans.push(span).is_err() {
                    state.dropped += 1;
                }
            }
        }
        self.device.write(blocks, start)
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        self.device.num_blocks()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obc2::fatsim::SparseDisk;

    #[test]
    fn it_counts_and_records_only_while_armed() {
        let log: WriteLog<_, 4> = WriteLog::new(SparseDisk::blank(64));
        let block = [Block::new()];
        log.write(&block, BlockIdx(3)).unwrap();
        assert_eq!(log.counters().writes, 1);
        log.with_spans(|spans| assert!(spans.is_empty()));

        log.arm();
        assert_eq!(log.counters(), Counters::default());
        log.write(&block, BlockIdx(7)).unwrap();
        log.read(&mut [Block::new(), Block::new()], BlockIdx(0)).unwrap();
        assert_eq!(log.counters(), Counters { reads: 1, read_blocks: 2, writes: 1, written_blocks: 1 });
        assert_eq!(log.writes_touching(7), 1);
        assert_eq!(log.writes_touching(8), 0);
    }

    #[test]
    fn an_overrun_is_counted_rather_than_lost() {
        let log: WriteLog<_, 2> = WriteLog::new(SparseDisk::blank(64));
        log.arm();
        for lba in 0..5 {
            log.write(&[Block::new()], BlockIdx(lba)).unwrap();
        }
        log.with_spans(|spans| assert_eq!(spans.len(), 2));
        assert_eq!(log.dropped(), 3);
        assert_eq!(log.counters().writes, 5);
    }

    #[test]
    fn a_multi_block_span_covers_its_whole_run() {
        let span = Span { start: 100, blocks: 8 };
        assert!(span.contains(100));
        assert!(span.contains(107));
        assert!(!span.contains(108));
        assert!(!span.contains(99));
    }
}
