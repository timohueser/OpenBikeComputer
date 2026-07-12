//! Install-engine sequencing tests (host, mock IO) — the safety property of the DFU epic (#615).
//!
//! Every test drives `obc_dfu::engine::run` against a `MockIo` that models the SD card (a block
//! map), the RRAM app slot, and the BOOT_STATE page, logging every operation. The assertions pin
//! the *ordering* (verify strictly before the first slot write, state transition strictly after
//! the last readback), the retry counts, the header-skip/padding byte math, and every failure
//! edge — power loss included.

use obc_dfu::engine::{run, InstallIo, IoError, Outcome, Phase, Slot, FLASH_RETRIES, PAD_BYTE};
use obc_dfu::{BootState, Extent, ImageHeader, LastOutcome, OutcomeKind, StagedRef, PAGE_LEN};
use std::collections::BTreeMap;

const BLOCK: usize = 512;
const HEADER_LEN: usize = 64;
/// A small app slot so tests stay fast; the engine takes it as a parameter.
const SLOT: Slot = Slot { base: 0x8000, len: 16 * 1024 };
/// What the mock's flash model is filled with before any write — distinguishable from both
/// image bytes and the 0xFF pad.
const FLASH_BLANK: u8 = 0xAA;
/// The engine's staging buffer, sized like the bootloader's.
const BUF_LEN: usize = 4096;

// ---------------------------------------------------------------------------- mock IO

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    ReadBlocks {
        start: u32,
        blocks: u32,
    },
    WriteLines {
        addr: u32,
        len: u32,
    },
    ReadFlash,
    /// The written state's tag (decoded back out of the page bytes).
    WriteState(&'static str),
}

struct MockIo {
    /// The SD card: absolute block index → block content.
    disk: BTreeMap<u32, [u8; BLOCK]>,
    /// The RRAM app slot, `SLOT.len` bytes at `SLOT.base`.
    flash: Vec<u8>,
    /// The BOOT_STATE page bytes.
    state_page: Vec<u8>,
    ops: Vec<Op>,
    /// Power-loss model: the write (write_lines + write_state, counted together) that dies
    /// mid-operation. The dying write applies a *partial* prefix (a torn write), then every
    /// later operation fails — the "device is off" state.
    kill_at_write: Option<usize>,
    writes_done: usize,
    dead: bool,
    /// Fail the Nth read_blocks call (transient card error model).
    fail_read_at: Option<usize>,
    reads_done: usize,
    /// Corrupt every write_lines (flip the first byte) — a flash that never takes the data.
    corrupt_writes: bool,
}

impl MockIo {
    fn new(state: &BootState) -> MockIo {
        let page = state.encode();
        let mut state_page = vec![0u8; PAGE_LEN];
        state_page[..page.len()].copy_from_slice(page.as_bytes());
        MockIo {
            disk: BTreeMap::new(),
            flash: vec![FLASH_BLANK; SLOT.len as usize],
            state_page,
            ops: Vec::new(),
            kill_at_write: None,
            writes_done: 0,
            dead: false,
            fail_read_at: None,
            reads_done: 0,
            corrupt_writes: false,
        }
    }

    /// Lay `bytes` onto the disk across `extents` (in order), padding the final block with 0xEE
    /// slack — exactly how a FAT file sits in its cluster chain.
    fn load_file(&mut self, bytes: &[u8], extents: &[Extent]) {
        let mut off = 0usize;
        for e in extents {
            for b in 0..e.blocks {
                let mut block = [0xEEu8; BLOCK];
                if off < bytes.len() {
                    let n = BLOCK.min(bytes.len() - off);
                    block[..n].copy_from_slice(&bytes[off..off + n]);
                    off += n;
                }
                self.disk.insert(e.start_block + b, block);
            }
        }
        assert!(off >= bytes.len(), "extents must cover the file: {} of {} placed", off, bytes.len());
    }

    /// The decoded BOOT_STATE page — what the next boot would see.
    fn state(&self) -> BootState {
        BootState::decode(&self.state_page)
    }

    fn count_write_lines(&self) -> usize {
        self.ops.iter().filter(|o| matches!(o, Op::WriteLines { .. })).count()
    }

    /// "Power cycle" the killed mock: alive again, op log cleared, disk/flash/state kept.
    fn power_cycle(&mut self) {
        self.dead = false;
        self.kill_at_write = None;
        self.writes_done = 0;
        self.ops.clear();
    }
}

impl InstallIo for MockIo {
    fn read_blocks(&mut self, start_block: u32, buf: &mut [u8]) -> Result<(), IoError> {
        assert!(!buf.is_empty() && buf.len().is_multiple_of(BLOCK), "engine must read whole blocks");
        if self.dead {
            return Err(IoError);
        }
        self.reads_done += 1;
        if self.fail_read_at == Some(self.reads_done) {
            return Err(IoError);
        }
        let blocks = (buf.len() / BLOCK) as u32;
        self.ops.push(Op::ReadBlocks { start: start_block, blocks });
        for i in 0..blocks {
            let src = self.disk.get(&(start_block + i)).ok_or(IoError)?;
            buf[i as usize * BLOCK..(i as usize + 1) * BLOCK].copy_from_slice(src);
        }
        Ok(())
    }

    fn write_lines(&mut self, addr: u32, data: &[u8]) -> Result<(), IoError> {
        assert!(addr.is_multiple_of(16), "RRAM writes must be line-aligned");
        assert!(!data.is_empty() && data.len().is_multiple_of(16), "RRAM writes are whole 16-byte lines");
        let off = (addr - SLOT.base) as usize;
        assert!(off + data.len() <= self.flash.len(), "write past the app slot: {addr:#x}+{}", data.len());
        if self.dead {
            return Err(IoError);
        }
        self.writes_done += 1;
        if self.kill_at_write == Some(self.writes_done) {
            // Torn write: half the lines (rounded down to a whole line) land, then lights out.
            let torn = data.len() / 2 / 16 * 16;
            self.flash[off..off + torn].copy_from_slice(&data[..torn]);
            self.dead = true;
            return Err(IoError);
        }
        self.ops.push(Op::WriteLines { addr, len: data.len() as u32 });
        let mut written = data.to_vec();
        if self.corrupt_writes {
            written[0] ^= 0x01;
        }
        self.flash[off..off + data.len()].copy_from_slice(&written);
        Ok(())
    }

    fn read_flash(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), IoError> {
        if self.dead {
            return Err(IoError);
        }
        self.ops.push(Op::ReadFlash);
        let off = (addr - SLOT.base) as usize;
        buf.copy_from_slice(&self.flash[off..off + buf.len()]);
        Ok(())
    }

    fn write_state(&mut self, state: &BootState) -> Result<(), IoError> {
        if self.dead {
            return Err(IoError);
        }
        let page = state.encode();
        self.writes_done += 1;
        if self.kill_at_write == Some(self.writes_done) {
            // Torn page write: a prefix of the new blob over the old page (front-to-back
            // 16-byte lines, like the RRAMC) — must decode to Idle via the CRC frame.
            let torn = page.len() / 2 / 16 * 16;
            self.state_page[..torn].copy_from_slice(&page.as_bytes()[..torn]);
            self.dead = true;
            return Err(IoError);
        }
        let tag = match state {
            BootState::Idle { .. } => "idle",
            BootState::Armed { .. } => "armed",
            BootState::Trial { .. } => "trial",
        };
        self.ops.push(Op::WriteState(tag));
        self.state_page.fill(0);
        self.state_page[..page.len()].copy_from_slice(page.as_bytes());
        Ok(())
    }

    fn progress(&mut self, _phase: Phase, _done: u32, _total: u32) {}
}

// ---------------------------------------------------------------------------- fixtures

/// A deterministic pseudo-random image of `len` bytes.
fn image(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u32).wrapping_mul(2654435761).to_le_bytes()[1]).collect()
}

/// Wrap `img` into an OBCU container file and a consistent `StagedRef` over `extents` — the
/// extent chain covers the WHOLE file (header + image), as the armer resolves `UPDATE.BIN`.
fn stage(img: &[u8], version: &str, extents: &[Extent]) -> (Vec<u8>, StagedRef) {
    let header = ImageHeader::new(img, version);
    let mut file = header.encode().to_vec();
    file.extend_from_slice(img);
    let total_blocks: u32 = extents.iter().map(|e| e.blocks).sum();
    assert!(
        (total_blocks as usize) * BLOCK >= file.len(),
        "test fixture: extents too short for the file ({total_blocks} blocks < {} bytes)",
        file.len()
    );
    let staged = StagedRef::new(header, header.image_len, header.image_crc32, extents).expect("consistent stage");
    (file, staged)
}

/// Extent chain with irregular run lengths covering `file_len` bytes (exercises the chain walk).
fn chain_for(file_len: usize, first_block: u32) -> Vec<Extent> {
    let need = file_len.div_ceil(BLOCK) as u32;
    let mut out = Vec::new();
    let mut placed = 0u32;
    let mut at = first_block;
    let mut run = 3u32; // 3, 1, 5, 3, 1, 5, ...
    while placed < need {
        let blocks = run.min(need - placed);
        out.push(Extent { start_block: at, blocks });
        placed += blocks;
        at += blocks + 7; // gaps between runs — fragmentation
        run = match run {
            3 => 1,
            1 => 5,
            _ => 3,
        };
    }
    out
}

/// An `Armed` fixture: image on disk, staged record, optional rollback snapshot also on disk.
fn armed(img_len: usize, with_rollback: bool) -> (MockIo, BootState, Vec<u8>, StagedRef) {
    let img = image(img_len);
    let extents = chain_for(img_len + HEADER_LEN, 1000);
    let (file, update) = stage(&img, "v2.0.0-new", &extents);
    let rollback = if with_rollback {
        let rb_img = image(3333);
        let rb_extents = chain_for(rb_img.len() + HEADER_LEN, 90_000);
        let (rb_file, rb) = stage(&rb_img, "v1.0.0-old", &rb_extents);
        Some((rb_file, rb))
    } else {
        None
    };
    let state = BootState::Armed { generation: 7, update, rollback: rollback.as_ref().map(|(_, r)| *r) };
    let mut io = MockIo::new(&state);
    io.load_file(&file, &extents);
    if let Some((rb_file, rb)) = &rollback {
        io.load_file(rb_file, rb.extents());
    }
    (io, state, img, update)
}

fn run_engine(io: &mut MockIo, state: &BootState) -> Outcome {
    let mut buf = [0u8; BUF_LEN];
    run(state, &SLOT, io, &mut buf)
}

/// Assert the app slot holds exactly `img`, tail-padded with 0xFF to a 16-byte line, and the
/// rest of the slot untouched (still the blank pattern) — the header-skip/padding contract.
fn assert_flash_is(io: &MockIo, img: &[u8]) {
    assert_eq!(&io.flash[..img.len()], img, "slot bytes must be exactly the raw image (header skipped)");
    let padded = img.len().div_ceil(16) * 16;
    assert!(io.flash[img.len()..padded].iter().all(|&b| b == PAD_BYTE), "tail must be 0xFF-padded to a line");
    assert!(io.flash[padded..].iter().all(|&b| b == FLASH_BLANK), "nothing past the padded image may be written");
}

// ---------------------------------------------------------------------------- the matrix

/// Happy path: verify → flash → readback → Trial, in that strict order, and the flashed bytes
/// are exactly the raw image (the 64-byte container header skipped, tail padded with 0xFF).
#[test]
fn happy_path_ordering_and_bytes() {
    // 9001: multi-chunk (3 buffer fills) and not a multiple of 16 (real tail padding).
    let (mut io, state, img, update) = armed(9001, true);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_flash_is(&io, &img);

    // Ordering: no slot write before the last verify read; the state write is the very last
    // operation and follows the last readback.
    let first_write = io.ops.iter().position(|o| matches!(o, Op::WriteLines { .. })).expect("flashed");
    let verify_reads: Vec<usize> =
        io.ops.iter().enumerate().filter_map(|(i, o)| matches!(o, Op::ReadBlocks { .. }).then_some(i)).collect();
    // The verify pass must have streamed the whole file before the first write: at least one
    // read precedes it, and the bytes read before the first write cover header + image.
    let bytes_before: u32 = io.ops[..first_write]
        .iter()
        .filter_map(|o| match o {
            Op::ReadBlocks { blocks, .. } => Some(*blocks * BLOCK as u32),
            _ => None,
        })
        .sum();
    assert!(bytes_before as usize >= HEADER_LEN + img.len(), "full verify stream before the first slot write");
    assert!(verify_reads.first().unwrap() < &first_write);
    // Last op = the Trial state write, after the last readback.
    assert_eq!(io.ops.last(), Some(&Op::WriteState("trial")));
    let last_readback = io.ops.iter().rposition(|o| matches!(o, Op::ReadFlash)).expect("readback ran");
    assert_eq!(last_readback, io.ops.len() - 2);

    // The written Trial carries the generation, the installed header, and the rollback ref.
    match io.state() {
        BootState::Trial { generation, installed, rollback } => {
            assert_eq!(generation, 7);
            assert_eq!(installed, update.header);
            assert!(rollback.is_some(), "rollback snapshot must ride into Trial");
        }
        s => panic!("expected Trial, got {s:?}"),
    }
}

/// A corrupt staged image (CRC mismatch) is rejected with ZERO slot writes and the arm cleared
/// to Idle — a bad stage never costs the running firmware (epic invariant 1).
#[test]
fn verify_crc_fail_writes_nothing() {
    let (mut io, state, _img, _update) = armed(9001, true);
    // Flip one image byte on card (past the header, mid-file).
    let key = *io.disk.keys().nth(2).unwrap();
    io.disk.get_mut(&key).unwrap()[100] ^= 0xFF;

    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0, "verify failure must not touch the app slot");
    assert!(io.flash.iter().all(|&b| b == FLASH_BLANK));
    // Arm cleared; the outgoing image's header (from the rollback snapshot) is carried forward,
    // and the terminal write records StageRejected against the arm's generation (7).
    match io.state() {
        BootState::Idle { installed, last_outcome } => {
            assert!(installed.is_some(), "outgoing header carried into Idle");
            assert_eq!(last_outcome, Some(LastOutcome { kind: OutcomeKind::StageRejected, generation: 7 }));
        }
        s => panic!("expected Idle, got {s:?}"),
    }
}

/// An embedded OBCU header on card that differs from the armed record is a mismatch too (the
/// blocks are not the image this arm described), rejected before any read of the image body.
#[test]
fn verify_foreign_header_rejected() {
    let (mut io, state, _img, _update) = armed(2000, false);
    // Overwrite the on-card header with a different (self-consistent!) one.
    let other = ImageHeader::new(&image(2000), "v9.9.9-foreign").encode();
    let first = *io.disk.keys().next().unwrap();
    io.disk.get_mut(&first).unwrap()[..HEADER_LEN].copy_from_slice(&other);

    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0);
    assert_eq!(
        io.state(),
        BootState::Idle {
            installed: None,
            last_outcome: Some(LastOutcome { kind: OutcomeKind::StageRejected, generation: 7 })
        },
        "no rollback ⇒ Idle carries no header, but records the reject"
    );
}

/// A chain that runs out before header+image is a deterministic bad stage, not an SD error.
#[test]
fn verify_truncated_chain_rejected() {
    let img = image(5000);
    let extents = chain_for(img.len() + HEADER_LEN, 1000);
    let (file, update) = stage(&img, "v2.0.0", &extents);
    // Arm with a chain one extent short — but the record still self-consistent.
    let short = &extents[..extents.len() - 1];
    let staged_short = StagedRef::new(update.header, update.len, update.crc32, short).unwrap();
    let state = BootState::Armed { generation: 1, update: staged_short, rollback: None };
    let mut io = MockIo::new(&state);
    io.load_file(&file, &extents);

    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0);
    assert_eq!(
        io.state(),
        BootState::Idle {
            installed: None,
            last_outcome: Some(LastOutcome { kind: OutcomeKind::StageRejected, generation: 1 })
        }
    );
}

/// An image whose line-padded length exceeds the slot is rejected up front — the pad can never
/// write past the slot (and a slot-filling image that DOES fit is accepted).
#[test]
fn slot_bounds_gate() {
    // Fits exactly: len == slot.len (already a 16-multiple).
    let (mut io, state, img, _) = armed(SLOT.len as usize, false);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_eq!(&io.flash[..], &img[..], "a slot-filling image is written wall to wall");

    // Doesn't fit: len + pad > slot.len ⇒ rejected before any read or write.
    let (mut io, state, _, _) = armed(SLOT.len as usize + 1, false);
    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0);
}

/// A transient SD read error must NOT clear the arm (unlike a mismatch): outcome `SdError`,
/// state untouched, nothing written — the caller retries and the stage stays installable.
#[test]
fn sd_error_leaves_arm_intact() {
    for fail_at in [1, 3, 7] {
        let (mut io, state, _img, _update) = armed(9001, true);
        io.fail_read_at = Some(fail_at);
        assert_eq!(run_engine(&mut io, &state), Outcome::SdError, "read #{fail_at}");
        assert!(!io.ops.iter().any(|o| matches!(o, Op::WriteState(_))), "state must stay Armed");
        assert_eq!(io.state(), state, "arm untouched after a transient SD error");
    }
}

/// Power-loss sweep: kill the mock at every possible write (torn), power-cycle, re-run from
/// whatever the page now decodes to — the system converges to the new image in the slot with a
/// sane state (epic invariant 2: `Armed` is idempotent; a torn state page decodes to `Idle`
/// only after the slot already holds the verified image).
#[test]
fn power_loss_converges() {
    // Count the writes of a clean run first.
    let (mut io, state, img, update) = armed(9001, true);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    let total_writes = io.writes_done;
    assert!(total_writes > 3, "sweep needs several kill points, got {total_writes}");

    for kill in 1..=total_writes {
        let (mut io, state, ..) = armed(9001, true);
        io.kill_at_write = Some(kill);
        let first = run_engine(&mut io, &state);
        assert_ne!(first, Outcome::Installed, "kill at write {kill} cannot complete");

        // Power cycle: next boot decodes whatever the page holds and re-runs.
        io.power_cycle();
        let next_state = io.state();
        let second = run_engine(&mut io, &next_state);

        // Converged: the slot holds the full verified image...
        assert_flash_is(&io, &img);
        // ...and the state is sane: Trial (normal convergence) or Idle (the kill tore the
        // final Trial write itself — flash was already complete and verified by then).
        match (second, io.state()) {
            (Outcome::Installed, BootState::Trial { installed, .. }) => assert_eq!(installed, update.header),
            (Outcome::Jump, BootState::Idle { .. }) => {
                assert_eq!(kill, total_writes, "only tearing the final state write may land in Idle");
            }
            (o, s) => panic!("kill at {kill}: unexpected outcome {o:?} / state {s:?}"),
        }
    }
}

/// Readback that never matches: the flash pass runs 1 + FLASH_RETRIES times, then the engine
/// halts with `FlashError` and — critically — NO state write happened: the page still holds
/// `Armed`, so the next power cycle retries from scratch.
#[test]
fn readback_fail_retries_then_halts() {
    let (mut io, state, _img, _update) = armed(9001, false);
    io.corrupt_writes = true; // every write lands corrupted ⇒ readback can never match

    assert_eq!(run_engine(&mut io, &state), Outcome::FlashError);

    // Exactly 1 + FLASH_RETRIES full flash passes (each pass writes ceil(9001/4096) = 3 chunks).
    let chunks_per_pass = 9001usize.div_ceil(BUF_LEN);
    assert_eq!(io.count_write_lines(), (1 + FLASH_RETRIES as usize) * chunks_per_pass, "flash retried exactly 3x");
    assert!(!io.ops.iter().any(|o| matches!(o, Op::WriteState(_))), "no state write on a failed install");
    assert_eq!(io.state(), state, "state must still be Armed so the next boot retries");
}

/// Rollback: same engine, source = the snapshot extents; ends in `Idle { installed: rollback }`
/// + reset (no second trial for the known-good image).
#[test]
fn rollback_path() {
    let rb_img = image(7003);
    let rb_extents = chain_for(rb_img.len() + HEADER_LEN, 50_000);
    let (rb_file, snapshot) = stage(&rb_img, "v1.0.0-known-good", &rb_extents);
    let trial_hdr = ImageHeader::new(&image(9001), "v2.0.0-bad");
    let state = BootState::Trial { generation: 4, installed: trial_hdr, rollback: Some(snapshot) };
    let mut io = MockIo::new(&state);
    io.load_file(&rb_file, &rb_extents);

    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_flash_is(&io, &rb_img);
    assert_eq!(io.ops.last(), Some(&Op::WriteState("idle")));
    match io.state() {
        BootState::Idle { installed, last_outcome } => {
            assert_eq!(installed, Some(snapshot.header));
            // The rollback restored the snapshot — recorded as RolledBack against the arm's gen (4).
            assert_eq!(last_outcome, Some(LastOutcome { kind: OutcomeKind::RolledBack, generation: 4 }));
        }
        s => panic!("expected Idle, got {s:?}"),
    }
}

/// A rollback whose snapshot is itself corrupt must NOT flash garbage over the (running,
/// fully-flashed) trial image: it clears to Idle and keeps what's in the slot.
#[test]
fn rollback_bad_snapshot_keeps_trial_image() {
    let rb_img = image(7003);
    let rb_extents = chain_for(rb_img.len() + HEADER_LEN, 50_000);
    let (rb_file, snapshot) = stage(&rb_img, "v1.0.0", &rb_extents);
    let trial_hdr = ImageHeader::new(&image(9001), "v2.0.0-trial");
    let state = BootState::Trial { generation: 4, installed: trial_hdr, rollback: Some(snapshot) };
    let mut io = MockIo::new(&state);
    io.load_file(&rb_file, &rb_extents);
    // Corrupt the snapshot on card.
    let key = *io.disk.keys().nth(1).unwrap();
    io.disk.get_mut(&key).unwrap()[9] ^= 0xFF;

    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0, "a bad snapshot must not overwrite the running image");
    match io.state() {
        BootState::Idle { installed, last_outcome } => {
            assert_eq!(installed, Some(trial_hdr), "the trial image is accepted");
            // The trial image stuck (snapshot unusable) — recorded as Installed against the gen (4).
            assert_eq!(last_outcome, Some(LastOutcome { kind: OutcomeKind::Installed, generation: 4 }));
        }
        s => panic!("expected Idle, got {s:?}"),
    }
}

/// AcceptAndClear (unconfirmed trial, no snapshot — the first-install case): write Idle with the
/// running image's header and jump; no SD, no slot writes.
#[test]
fn accept_and_clear() {
    let installed = ImageHeader::new(&image(1234), "v1.0.0-first");
    let state = BootState::Trial { generation: 1, installed, rollback: None };
    let mut io = MockIo::new(&state);

    assert_eq!(run_engine(&mut io, &state), Outcome::Jump);
    assert_eq!(io.ops, vec![Op::WriteState("idle")], "exactly one op: the Idle write");
    assert_eq!(
        io.state(),
        BootState::Idle {
            installed: Some(installed),
            // First-install trial accepted — recorded as Installed against the arm's gen (1).
            last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 1 })
        }
    );
}

/// Idle: nothing pending, nothing done.
#[test]
fn idle_jumps_untouched() {
    let state = BootState::Idle { installed: None, last_outcome: None };
    let mut io = MockIo::new(&state);
    assert_eq!(run_engine(&mut io, &state), Outcome::Jump);
    assert!(io.ops.is_empty());
}

/// Sub-chunk image (single buffer fill, tail shorter than a block) — the small-image edge of the
/// header-skip + padding math.
#[test]
fn tiny_image_bytes_exact() {
    let (mut io, state, img, _) = armed(100, false);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_flash_is(&io, &img);
}
