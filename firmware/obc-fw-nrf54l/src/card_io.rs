//! Raw sEMMC helper for the flat store: the aligned DMA bounce buffer.

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
