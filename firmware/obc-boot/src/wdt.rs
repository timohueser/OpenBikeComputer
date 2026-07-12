//! The hardware watchdog across the DFU boot chain (DR1, #729).
//!
//! The app runs a 24 s WDT (`obc-fw-nrf54l/src/main.rs`, #349) and the arm path enters this
//! bootloader through a warm `SCB::sys_reset()` — which carries the running, unstoppable dog
//! over. Left unfed, that dog would cut a slow install down mid-flash (the SD backoffs alone
//! reach 8 s per lap, and a near-max image plus flash-retry passes runs well past 24 s), and
//! would silently turn the deliberate parks — the SOS halt, the card-retry loop — into reset
//! storms. On the other side, a **cold power-on** with `Armed` persisted runs the whole chain
//! dog-*less*: if the freshly installed trial image wedges before the app configures its own
//! WDT, nothing ever resets the device and the designed rollback never fires.
//!
//! [`BootDog`] closes both gaps using the app's exact config
//! ([`obc_dfu::WDT_TIMEOUT_TICKS`] — the contract note there is normative):
//!
//! - **Adopt** a dog that is already running (the warm-reset arm path) and pet it through the
//!   install. embassy-nrf's `Watchdog::try_new` reconstructs the handle for an already-running
//!   watchdog when the whole config matches — the same mechanism the app itself relies on to
//!   adopt the dog across the trial jump.
//! - **Stay idle** on a cold boot: no dog is started just to run the install, so the parks
//!   stay power-cycle parks — exactly the documented recovery story (README LED table).
//! - **Start** the dog immediately before the trial jump ([`BootDog::start_for_trial`]), so a
//!   trial image that wedges pre-WDT-setup resets back into this bootloader, which then reads
//!   the unconfirmed `Trial` and rolls back (epic #615 invariant 3). The `Idle` fast path
//!   never constructs a `BootDog` at all — a normal boot stays byte-identical.

use embassy_nrf::peripherals::WDT0;
use embassy_nrf::{wdt, Peri};

/// The app's exact WDT config — field-for-field the value `obc-fw-nrf54l/src/main.rs` builds
/// (see the contract on [`obc_dfu::WDT_TIMEOUT_TICKS`]): 24 s timeout, pause under a debug
/// halt (so probe-rs can flash with the dog live), the default run-through-sleep, and — via
/// the `N = 1` at both `try_new` call sites — a single pet handle (RREN = bit 0). Any field
/// differing means adoption fails on *both* sides of the chain.
fn app_wdt_config() -> wdt::Config {
    let mut cfg = wdt::Config::default();
    cfg.timeout_ticks = obc_dfu::WDT_TIMEOUT_TICKS;
    cfg.action_during_debug_halt = wdt::HaltConfig::Pause;
    cfg
}

/// The bootloader's view of the watchdog. Constructed once at the top of the slow path; the
/// `Idle` fast path returns before it exists.
pub struct BootDog {
    /// The pet handle — `Some` once a matching running dog was adopted (warm-reset entry) or
    /// started for the trial jump.
    handle: Option<wdt::WatchdogHandle>,
    /// The untouched peripheral, held back for [`start_for_trial`](BootDog::start_for_trial) —
    /// `Some` only when no dog was running at entry (the cold-boot case).
    idle: Option<Peri<'static, WDT0>>,
}

impl BootDog {
    /// Take stock of the watchdog at slow-path entry. Never *starts* a dog — a running one is
    /// adopted (and pet once by embassy's `try_new`), an idle peripheral is held for the trial
    /// jump, and a foreign-config dog (an older image's — this codebase's config is constant)
    /// is left unfed, mirroring the app: nothing can feed it, its stale period fires once, and
    /// the next boot re-enters here clean with the `Armed` record intact (`obc-fw-nrf54l/src/
    /// main.rs`'s WDT notes — a dog-fired reset, unlike a soft reset, does not carry the dog
    /// over).
    pub fn take(wdt0: Peri<'static, WDT0>) -> BootDog {
        // `Config::try_new` reads the live registers: `None` means the watchdog is not
        // running (RUNSTATUS clear) — the cold-boot case.
        if wdt::Config::try_new(&wdt0).is_none() {
            return BootDog { handle: None, idle: Some(wdt0) };
        }
        match wdt::Watchdog::try_new::<_, 1>(wdt0, app_wdt_config()) {
            Ok((_wdt, [handle])) => {
                #[cfg(feature = "rtt")]
                defmt::info!("obc-boot: adopted the app's running WDT — petting through the install");
                BootDog { handle: Some(handle), idle: None }
            }
            Err(_) => {
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: WDT running with a foreign config — cannot feed it; expect one reset");
                BootDog { handle: None, idle: None }
            }
        }
    }

    /// Pet the adopted/started dog, if there is one. Called from every long-running stretch:
    /// each engine progress chunk, each SD retry/backoff lap, and each SOS cycle — all far
    /// inside the 24 s window. A single register write; free to call when no dog is live.
    pub fn pet(&mut self) {
        if let Some(h) = self.handle.as_mut() {
            h.pet();
        }
    }

    /// Gap B (#729): guarantee the imminent **trial boot** runs under the dog. A no-op when
    /// one is already live (the warm-reset path — the jump simply inherits it); on a cold boot
    /// this starts the dog with the app's exact config, which `main.rs`'s own `try_new` then
    /// adopts cleanly once the trial image is healthy enough to get there. If the trial wedges
    /// first, the dog resets into this bootloader → unconfirmed `Trial` → rollback.
    pub fn start_for_trial(&mut self) {
        if self.handle.is_some() {
            return;
        }
        // `idle` is `None` only in the foreign-config case — nothing can be started or fed
        // then, and `take` already logged it.
        let Some(wdt0) = self.idle.take() else { return };
        if let Ok((_wdt, [handle])) = wdt::Watchdog::try_new::<_, 1>(wdt0, app_wdt_config()) {
            #[cfg(feature = "rtt")]
            defmt::info!("obc-boot: WDT started for the trial boot (24 s)");
            self.handle = Some(handle);
        }
        // The Err arm is unreachable in practice (`take` saw the dog idle and nothing else in
        // this crate starts it); leaving `handle` empty is the total, panic-free fallback.
    }
}
