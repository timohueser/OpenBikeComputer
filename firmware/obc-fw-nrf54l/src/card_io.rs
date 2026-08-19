//! Raw sEMMC helpers shared by the flat store and the temporary FAT-only remnants.
//!
//! This module owns the one aligned DMA bounce and the optional physical-read census. Neither is
//! a filesystem concern: the flat store talks to the raw card directly and must keep both after
//! the FAT map path is deleted.

use crate::semmc::BLOCK_BYTES;

const BOUNCE_BLOCKS: usize = 4;
pub(crate) const BOUNCE_BYTES: usize = BOUNCE_BLOCKS * BLOCK_BYTES;

#[repr(C, align(4))]
struct Bounce([u8; BOUNCE_BYTES]);

static mut BOUNCE: Bounce = Bounce([0; BOUNCE_BYTES]);
static WARNED_BOUNCE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn warn_bounce(addr: usize) {
    if !WARNED_BOUNCE.swap(true, core::sync::atomic::Ordering::Relaxed) {
        defmt::warn!("SD: misaligned block buffer at 0x{=usize:08x} — bouncing (throughput cost)", addr);
    }
}

/// Lend the one aligned transfer buffer while the caller holds `flpr_mux::with_storage`.
///
/// # Safety
/// The caller must be inside the mux's non-reentrant storage closure and must not nest another
/// bounce use.
pub(crate) unsafe fn with_bounce<R>(addr: usize, f: impl FnOnce(&mut [u8]) -> R) -> R {
    warn_bounce(addr);
    // SAFETY: upheld by the caller contract above.
    let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
    f(&mut bounce.0)
}

#[cfg(feature = "sd-bench")]
fn command_shape(addr: usize, blocks: usize) -> (usize, usize) {
    if addr.is_multiple_of(4) {
        (usize::from(blocks != 0), usize::from(blocks == 1))
    } else {
        (blocks.div_ceil(BOUNCE_BLOCKS), usize::from(blocks % BOUNCE_BLOCKS == 1))
    }
}

#[cfg(feature = "sd-bench")]
#[derive(Clone, Copy)]
pub(crate) struct ReadPerf {
    pub(crate) us: u32,
    pub(crate) commands: u32,
    pub(crate) blocks: u32,
    pub(crate) single_commands: u32,
    pub(crate) multi_commands: u32,
}

#[cfg(feature = "sd-bench")]
impl ReadPerf {
    pub(crate) const ZERO: Self = Self { us: 0, commands: 0, blocks: 0, single_commands: 0, multi_commands: 0 };

    pub(crate) fn since(self, before: Self) -> Self {
        Self {
            us: self.us.wrapping_sub(before.us),
            commands: self.commands.wrapping_sub(before.commands),
            blocks: self.blocks.wrapping_sub(before.blocks),
            single_commands: self.single_commands.wrapping_sub(before.single_commands),
            multi_commands: self.multi_commands.wrapping_sub(before.multi_commands),
        }
    }

    pub(crate) fn add_assign(&mut self, other: Self) {
        self.us = self.us.wrapping_add(other.us);
        self.commands = self.commands.wrapping_add(other.commands);
        self.blocks = self.blocks.wrapping_add(other.blocks);
        self.single_commands = self.single_commands.wrapping_add(other.single_commands);
        self.multi_commands = self.multi_commands.wrapping_add(other.multi_commands);
    }
}

#[cfg(feature = "sd-bench")]
static READ_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_COMMANDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_BLOCKS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_SINGLE_COMMANDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_MULTI_COMMANDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "sd-bench")]
pub(crate) fn read_perf_snapshot() -> ReadPerf {
    use core::sync::atomic::Ordering::Relaxed;
    ReadPerf {
        us: READ_US.load(Relaxed),
        commands: READ_COMMANDS.load(Relaxed),
        blocks: READ_BLOCKS.load(Relaxed),
        single_commands: READ_SINGLE_COMMANDS.load(Relaxed),
        multi_commands: READ_MULTI_COMMANDS.load(Relaxed),
    }
}

#[cfg(feature = "sd-bench")]
pub(crate) fn note_read_perf(started: embassy_time::Instant, addr: usize, blocks: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    let (commands, singles) = command_shape(addr, blocks);
    let elapsed = started.elapsed().as_micros().min(u64::from(u32::MAX)) as u32;
    READ_US.fetch_add(elapsed, Relaxed);
    READ_COMMANDS.fetch_add(commands as u32, Relaxed);
    READ_BLOCKS.fetch_add(blocks as u32, Relaxed);
    READ_SINGLE_COMMANDS.fetch_add(singles as u32, Relaxed);
    READ_MULTI_COMMANDS.fetch_add((commands - singles) as u32, Relaxed);
}
