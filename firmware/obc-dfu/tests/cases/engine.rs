//! Install-engine sequencing tests. Every test drives `obc_dfu::engine::run` against a `MockIo`
//! that models the card, the RRAM app slot and the boot-state page, and logs every operation. The
//! assertions pin the ordering, the retry counts, the byte math, and every failure edge.

use obc_dfu::engine::{abandon_arm, run, InstallIo, IoError, Outcome, Phase, Slot, FLASH_RETRIES, PAD_BYTE};
use obc_dfu::{
    BootState, Extent, ImageHeader, LastOutcome, OutcomeKind, StagedRef, APP_SLOT_BASE, MAX_EXTENTS, MAX_IMAGE_LEN,
    PAGE_LEN,
};
use std::collections::BTreeMap;

const BLOCK: usize = 512;
const HEADER_LEN: usize = 64;
/// A small model slot, so the sequencing tests stay cheap. Its base is the real one, because
/// the IO mock turns addresses into offsets.
const SLOT: Slot = Slot { base: APP_SLOT_BASE, len: 16 * 1024 };
/// The slot the bootloader really hands in.
const APP_SLOT: Slot = Slot { base: APP_SLOT_BASE, len: MAX_IMAGE_LEN };
/// Fills the flash model before any write; distinct from both image bytes and the 0xFF pad.
const FLASH_BLANK: u8 = 0xAA;
const BUF_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    ReadBlocks { start: u32, blocks: u32 },
    WriteLines { addr: u32, len: u32 },
    ReadFlash,
    WriteState(&'static str),
}

struct MockIo {
    /// Absolute block index to block content.
    disk: BTreeMap<u32, [u8; BLOCK]>,
    flash: Vec<u8>,
    state_page: Vec<u8>,
    ops: Vec<Op>,
    /// Power-loss model: the write that dies mid-operation. It applies a partial prefix, then
    /// every later operation fails.
    kill_at_write: Option<usize>,
    writes_done: usize,
    dead: bool,
    /// Fail the Nth read_blocks call: a transient card error.
    fail_read_at: Option<usize>,
    reads_done: usize,
    /// Fail every read_blocks once a slot write has happened: a card that dies after the flash
    /// pass began, which is not abandonable.
    fail_read_after_write: bool,
    /// Flip the first byte of every write: a flash that never takes the data.
    corrupt_writes: bool,
}

impl MockIo {
    fn new(state: &BootState, slot_len: u32) -> MockIo {
        let page = state.encode();
        let mut state_page = vec![0u8; PAGE_LEN];
        state_page[..page.len()].copy_from_slice(page.as_bytes());
        MockIo {
            disk: BTreeMap::new(),
            flash: vec![FLASH_BLANK; slot_len as usize],
            state_page,
            ops: Vec::new(),
            kill_at_write: None,
            writes_done: 0,
            dead: false,
            fail_read_at: None,
            reads_done: 0,
            fail_read_after_write: false,
            corrupt_writes: false,
        }
    }

    /// Lays `bytes` across `extents`, with slack in the final block, as an object sits in the
    /// extents it owns.
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

    fn state(&self) -> BootState {
        BootState::decode(&self.state_page)
    }

    fn count_write_lines(&self) -> usize {
        self.ops.iter().filter(|o| matches!(o, Op::WriteLines { .. })).count()
    }

    /// Alive again, op log cleared, disk, flash and state kept.
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
        if self.fail_read_after_write && self.writes_done > 0 {
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
            // A prefix of the new blob over the old page, like the RRAMC writes lines, so the
            // CRC frame must decode it to Idle.
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

fn image(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u32).wrapping_mul(2654435761).to_le_bytes()[1]).collect()
}

/// The extent chain covers the whole object, header included, as the armer resolves a package.
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

/// An extent chain with irregular run lengths and gaps between them, to exercise the chain walk.
/// It spreads the file over as many runs as the record holds, which is the worst case the installer
/// can be handed.
fn chain_for(file_len: usize, first_block: u32) -> Vec<Extent> {
    let need = file_len.div_ceil(BLOCK) as u32;
    let runs = (MAX_EXTENTS as u32).min(need);
    let mut out = Vec::new();
    let mut placed = 0u32;
    let mut at = first_block;
    for run in 0..runs {
        let left_after = runs - run - 1; // one block each for the runs still to come
        let blocks = ((need / runs) + run % 3).max(1).min(need - placed - left_after);
        out.push(Extent { start_block: at, blocks });
        placed += blocks;
        at += blocks + 7; // gaps between runs — fragmentation
    }
    // Whatever the uneven split left over rides on the last run.
    if placed < need {
        out.last_mut().expect("a non-empty file has at least one run").blocks += need - placed;
    }
    out
}

fn armed(img_len: usize, with_rollback: bool) -> (MockIo, BootState, Vec<u8>, StagedRef) {
    armed_in(SLOT, img_len, with_rollback)
}

fn armed_in(slot: Slot, img_len: usize, with_rollback: bool) -> (MockIo, BootState, Vec<u8>, StagedRef) {
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
    let mut io = MockIo::new(&state, slot.len);
    io.load_file(&file, &extents);
    if let Some((rb_file, rb)) = &rollback {
        io.load_file(rb_file, rb.extents());
    }
    (io, state, img, update)
}

fn run_engine(io: &mut MockIo, state: &BootState) -> Outcome {
    run_engine_in(io, state, &SLOT)
}

fn run_engine_in(io: &mut MockIo, state: &BootState, slot: &Slot) -> Outcome {
    let mut buf = [0u8; BUF_LEN];
    run(state, slot, io, &mut buf)
}

fn assert_flash_is(io: &MockIo, img: &[u8]) {
    assert_eq!(&io.flash[..img.len()], img, "slot bytes must be exactly the raw image (header skipped)");
    let padded = img.len().div_ceil(16) * 16;
    assert!(io.flash[img.len()..padded].iter().all(|&b| b == PAD_BYTE), "tail must be 0xFF-padded to a line");
    assert!(io.flash[padded..].iter().all(|&b| b == FLASH_BLANK), "nothing past the padded image may be written");
}

/// Verify, flash, readback, then Trial, in that strict order.
#[test]
fn happy_path_ordering_and_bytes() {
    // 9001 bytes: three buffer fills, and not a multiple of 16, so the tail really pads.
    let (mut io, state, img, update) = armed(9001, true);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_flash_is(&io, &img);

    let first_write = io.ops.iter().position(|o| matches!(o, Op::WriteLines { .. })).expect("flashed");
    let verify_reads: Vec<usize> =
        io.ops.iter().enumerate().filter_map(|(i, o)| matches!(o, Op::ReadBlocks { .. }).then_some(i)).collect();
    let bytes_before: u32 = io.ops[..first_write]
        .iter()
        .filter_map(|o| match o {
            Op::ReadBlocks { blocks, .. } => Some(*blocks * BLOCK as u32),
            _ => None,
        })
        .sum();
    assert!(bytes_before as usize >= HEADER_LEN + img.len(), "full verify stream before the first slot write");
    assert!(verify_reads.first().unwrap() < &first_write);
    assert_eq!(io.ops.last(), Some(&Op::WriteState("trial")));
    let last_readback = io.ops.iter().rposition(|o| matches!(o, Op::ReadFlash)).expect("readback ran");
    assert_eq!(last_readback, io.ops.len() - 2);

    match io.state() {
        BootState::Trial { generation, installed, rollback } => {
            assert_eq!(generation, 7);
            assert_eq!(installed, update.header);
            assert!(rollback.is_some(), "rollback snapshot must ride into Trial");
        }
        s => panic!("expected Trial, got {s:?}"),
    }
}

/// A bad stage must never cost the running firmware.
#[test]
fn verify_crc_fail_writes_nothing() {
    let (mut io, state, _img, _update) = armed(9001, true);
    let key = *io.disk.keys().nth(2).unwrap();
    io.disk.get_mut(&key).unwrap()[100] ^= 0xFF;

    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0, "verify failure must not touch the app slot");
    assert!(io.flash.iter().all(|&b| b == FLASH_BLANK));
    match io.state() {
        BootState::Idle { installed, last_outcome } => {
            assert!(installed.is_some(), "outgoing header carried into Idle");
            assert_eq!(last_outcome, Some(LastOutcome { kind: OutcomeKind::StageRejected, generation: 7 }));
        }
        s => panic!("expected Idle, got {s:?}"),
    }
}

/// A header on card that differs from the armed record means the blocks are not the image this
/// arm described.
#[test]
fn verify_foreign_header_rejected() {
    let (mut io, state, _img, _update) = armed(2000, false);
    // A different, but self-consistent, header.
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

/// A chain that runs out is a bad stage, not an SD error.
#[test]
fn verify_truncated_chain_rejected() {
    let img = image(5000);
    let extents = chain_for(img.len() + HEADER_LEN, 1000);
    let (file, update) = stage(&img, "v2.0.0", &extents);
    // One extent short, but the record is still self-consistent.
    let short = &extents[..extents.len() - 1];
    let staged_short = StagedRef::new(update.header, update.len, update.crc32, short).unwrap();
    let state = BootState::Armed { generation: 1, update: staged_short, rollback: None };
    let mut io = MockIo::new(&state, SLOT.len);
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

/// The pad must never write past the slot, and an image that fills the slot exactly is accepted.
#[test]
fn slot_bounds_gate() {
    let (mut io, state, img, _) = armed(SLOT.len as usize, false);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_eq!(&io.flash[..], &img[..], "a slot-filling image is written wall to wall");

    let (mut io, state, _, _) = armed(SLOT.len as usize + 1, false);
    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0);
}

/// `MAX_IMAGE_LEN` is the app slot, so the engine's two length gates coincide there: an image at
/// the cap installs wall to wall, and one byte more is rejected before anything is erased.
#[test]
fn an_image_at_the_cap_fills_the_app_slot() {
    let (mut io, state, img, _) = armed_in(APP_SLOT, MAX_IMAGE_LEN as usize, false);
    assert_eq!(run_engine_in(&mut io, &state, &APP_SLOT), Outcome::Installed);
    assert_eq!(&io.flash[..], &img[..], "a slot-filling image is written wall to wall");

    let (mut io, state, _, _) = armed_in(APP_SLOT, MAX_IMAGE_LEN as usize + 1, false);
    assert_eq!(run_engine_in(&mut io, &state, &APP_SLOT), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0);
}

/// A transient SD read error must not clear the arm. These failures are all in the verify pass,
/// so the arm stays abandonable.
#[test]
fn sd_error_leaves_arm_intact() {
    for fail_at in [1, 3, 7] {
        let (mut io, state, _img, _update) = armed(9001, true);
        io.fail_read_at = Some(fail_at);
        assert_eq!(run_engine(&mut io, &state), Outcome::SdError { pre_erase: true }, "read #{fail_at}");
        assert!(!io.ops.iter().any(|o| matches!(o, Op::WriteState(_))), "state must stay Armed");
        assert_eq!(io.state(), state, "arm untouched after a transient SD error");
    }
}

/// An SD error after the flash pass began is not abandonable: the slot may be half-written, so
/// the state stays `Armed`.
#[test]
fn sd_error_mid_flash_is_not_abandonable() {
    let (mut io, state, _img, _update) = armed(9001, true);
    io.fail_read_after_write = true;

    assert_eq!(run_engine(&mut io, &state), Outcome::SdError { pre_erase: false });
    assert!(io.count_write_lines() > 0, "the flash pass must have started (a slot write happened)");
    assert!(!io.ops.iter().any(|o| matches!(o, Op::WriteState(_))), "a touched slot must keep the arm");
    assert_eq!(io.state(), state, "arm untouched — a mid-flash card death is never abandoned");
}

/// After the driver's retry budget is exhausted it abandons the arm: cleared to `Idle`, the
/// outcome recorded, and the app slot never touched. Both arms, with and without a rollback.
#[test]
fn abandon_arm_clears_to_idle_and_boots_old_app() {
    for with_rollback in [true, false] {
        let (mut io, state, _img, _update) = armed(9001, with_rollback);

        io.fail_read_at = Some(1);
        assert_eq!(run_engine(&mut io, &state), Outcome::SdError { pre_erase: true });
        assert_eq!(io.count_write_lines(), 0, "nothing may be erased before the abandon");

        assert_eq!(abandon_arm(&state, &mut io), Outcome::ArmAbandoned);
        assert_eq!(io.count_write_lines(), 0, "the abandon must never touch the app slot");

        let expected_installed = match &state {
            BootState::Armed { rollback, .. } => rollback.as_ref().map(|r| r.header),
            _ => unreachable!(),
        };
        match io.state() {
            BootState::Idle { installed, last_outcome } => {
                assert_eq!(installed, expected_installed, "installed = the outgoing image's header");
                assert_eq!(
                    last_outcome,
                    Some(LastOutcome { kind: OutcomeKind::ArmAbandoned, generation: 7 }),
                    "the abandon is recorded so the next boot's verdict surfaces it"
                );
            }
            s => panic!("expected Idle after abandon, got {s:?}"),
        }
    }
}

/// A non-`Armed` abandon is a caller bug, and must stay total: no write, no panic.
#[test]
fn abandon_arm_on_non_armed_is_a_noop_jump() {
    let state = BootState::Idle { installed: None, last_outcome: None };
    let mut io = MockIo::new(&state, SLOT.len);
    assert_eq!(abandon_arm(&state, &mut io), Outcome::Jump);
    assert!(io.ops.is_empty(), "no writes on a non-Armed abandon");
}

/// Kill the mock at every possible write, power-cycle, and re-run from whatever the page decodes
/// to. The system must converge to the new image in the slot with a sane state.
#[test]
fn power_loss_converges() {
    let (mut io, state, img, update) = armed(9001, true);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    let total_writes = io.writes_done;
    assert!(total_writes > 3, "sweep needs several kill points, got {total_writes}");

    for kill in 1..=total_writes {
        let (mut io, state, ..) = armed(9001, true);
        io.kill_at_write = Some(kill);
        let first = run_engine(&mut io, &state);
        assert_ne!(first, Outcome::Installed, "kill at write {kill} cannot complete");

        io.power_cycle();
        let next_state = io.state();
        let second = run_engine(&mut io, &next_state);

        assert_flash_is(&io, &img);
        // Idle is sane only when the kill tore the final Trial write, after a complete flash.
        match (second, io.state()) {
            (Outcome::Installed, BootState::Trial { installed, .. }) => assert_eq!(installed, update.header),
            (Outcome::Jump, BootState::Idle { .. }) => {
                assert_eq!(kill, total_writes, "only tearing the final state write may land in Idle");
            }
            (o, s) => panic!("kill at {kill}: unexpected outcome {o:?} / state {s:?}"),
        }
    }
}

/// A readback that never matches halts with no state write, so the page still holds `Armed` and
/// the next power cycle retries from the start.
#[test]
fn readback_fail_retries_then_halts() {
    let (mut io, state, _img, _update) = armed(9001, false);
    io.corrupt_writes = true;

    assert_eq!(run_engine(&mut io, &state), Outcome::FlashError);

    let chunks_per_pass = 9001usize.div_ceil(BUF_LEN);
    assert_eq!(io.count_write_lines(), (1 + FLASH_RETRIES as usize) * chunks_per_pass, "flash retried exactly 3x");
    assert!(!io.ops.iter().any(|o| matches!(o, Op::WriteState(_))), "no state write on a failed install");
    assert_eq!(io.state(), state, "state must still be Armed so the next boot retries");
}

/// A rollback ends in `Idle`: the known-good image gets no second trial.
#[test]
fn rollback_path() {
    let rb_img = image(7003);
    let rb_extents = chain_for(rb_img.len() + HEADER_LEN, 50_000);
    let (rb_file, snapshot) = stage(&rb_img, "v1.0.0-known-good", &rb_extents);
    let trial_hdr = ImageHeader::new(&image(9001), "v2.0.0-bad");
    let state = BootState::Trial { generation: 4, installed: trial_hdr, rollback: Some(snapshot) };
    let mut io = MockIo::new(&state, SLOT.len);
    io.load_file(&rb_file, &rb_extents);

    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_flash_is(&io, &rb_img);
    assert_eq!(io.ops.last(), Some(&Op::WriteState("idle")));
    match io.state() {
        BootState::Idle { installed, last_outcome } => {
            assert_eq!(installed, Some(snapshot.header));
            assert_eq!(last_outcome, Some(LastOutcome { kind: OutcomeKind::RolledBack, generation: 4 }));
        }
        s => panic!("expected Idle, got {s:?}"),
    }
}

/// A corrupt snapshot must not flash garbage over the running trial image.
#[test]
fn rollback_bad_snapshot_keeps_trial_image() {
    let rb_img = image(7003);
    let rb_extents = chain_for(rb_img.len() + HEADER_LEN, 50_000);
    let (rb_file, snapshot) = stage(&rb_img, "v1.0.0", &rb_extents);
    let trial_hdr = ImageHeader::new(&image(9001), "v2.0.0-trial");
    let state = BootState::Trial { generation: 4, installed: trial_hdr, rollback: Some(snapshot) };
    let mut io = MockIo::new(&state, SLOT.len);
    io.load_file(&rb_file, &rb_extents);
    let key = *io.disk.keys().nth(1).unwrap();
    io.disk.get_mut(&key).unwrap()[9] ^= 0xFF;

    assert_eq!(run_engine(&mut io, &state), Outcome::StageRejected);
    assert_eq!(io.count_write_lines(), 0, "a bad snapshot must not overwrite the running image");
    match io.state() {
        BootState::Idle { installed, last_outcome } => {
            assert_eq!(installed, Some(trial_hdr), "the trial image is accepted");
            assert_eq!(last_outcome, Some(LastOutcome { kind: OutcomeKind::Installed, generation: 4 }));
        }
        s => panic!("expected Idle, got {s:?}"),
    }
}

/// An unconfirmed trial with no snapshot writes `Idle` and jumps: no card reads, no slot writes.
#[test]
fn accept_and_clear() {
    let installed = ImageHeader::new(&image(1234), "v1.0.0-first");
    let state = BootState::Trial { generation: 1, installed, rollback: None };
    let mut io = MockIo::new(&state, SLOT.len);

    assert_eq!(run_engine(&mut io, &state), Outcome::Jump);
    assert_eq!(io.ops, vec![Op::WriteState("idle")], "exactly one op: the Idle write");
    assert_eq!(
        io.state(),
        BootState::Idle {
            installed: Some(installed),
            last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 1 })
        }
    );
}

#[test]
fn idle_jumps_untouched() {
    let state = BootState::Idle { installed: None, last_outcome: None };
    let mut io = MockIo::new(&state, SLOT.len);
    assert_eq!(run_engine(&mut io, &state), Outcome::Jump);
    assert!(io.ops.is_empty());
}

/// A single buffer fill with a tail shorter than a block: the small edge of the padding math.
#[test]
fn tiny_image_bytes_exact() {
    let (mut io, state, img, _) = armed(100, false);
    assert_eq!(run_engine(&mut io, &state), Outcome::Installed);
    assert_flash_is(&io, &img);
}
