//! The hardware watchdog across the DFU boot chain.
//!
//! The arm path enters this bootloader through a warm `SCB::sys_reset()`, which carries the
//! app's running 24 s watchdog over. That dog cannot be stopped. Left unfed it cuts a slow
//! install off mid-flash, and it turns the deliberate parks (the SOS halt, the card-retry loop)
//! into reset storms. A cold power-on with `Armed` persisted has the opposite problem: the whole
//! chain runs dog-less, so a trial image that wedges before the app sets up its own WDT never
//! resets and the rollback never fires.
//!
//! [`BootDog`] uses the app's exact config ([`obc_dfu::WDT_TIMEOUT_TICKS`], whose contract note
//! is normative). It adopts a running dog and pets it through the install, stays idle on a cold
//! boot so the parks stay power-cycle parks, and starts the dog immediately before the trial
//! jump. The `Idle` fast path never constructs a `BootDog`.

use embassy_nrf::peripherals::WDT0;
use embassy_nrf::{wdt, Peri};

/// The app's exact WDT config, field for field the value `obc-fw-nrf54l/src/main.rs` builds:
/// 24 s timeout, pause under a debug halt so probe-rs can flash with the dog live, and `N = 1`
/// at both `try_new` call sites for a single pet handle. Any field that differs makes adoption
/// fail on both sides of the chain.
fn app_wdt_config() -> wdt::Config {
    let mut cfg = wdt::Config::default();
    cfg.timeout_ticks = obc_dfu::WDT_TIMEOUT_TICKS;
    cfg.action_during_debug_halt = wdt::HaltConfig::Pause;
    cfg
}

/// The bootloader's view of the watchdog. The `Idle` fast path returns before it exists.
pub struct BootDog {
    /// The pet handle. `Some` once a running dog was adopted or one was started for the trial.
    handle: Option<wdt::WatchdogHandle>,
    /// The untouched peripheral, held back for the trial jump. `Some` only on a cold boot.
    idle: Option<Peri<'static, WDT0>>,
}

impl BootDog {
    /// Take stock of the watchdog at slow-path entry. It never starts a dog: a running one is
    /// adopted, an idle peripheral is held for the trial jump, and a dog with a foreign config
    /// is left unfed. Nothing can feed a foreign dog. Its stale period fires once, and because a
    /// dog-fired reset does not carry the dog over, the next boot enters here clean with the
    /// `Armed` record intact.
    pub fn take(wdt0: Peri<'static, WDT0>) -> BootDog {
        // `Config::try_new` reads the live registers. `None` means RUNSTATUS is clear: a cold boot.
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

    /// Pet the adopted or started dog, if there is one. Safe to call when no dog is live.
    pub fn pet(&mut self) {
        if let Some(h) = self.handle.as_mut() {
            h.pet();
        }
    }

    /// Make sure the imminent trial boot runs under the dog. A no-op when one is already live.
    /// On a cold boot it starts the dog with the app's exact config, which the app's own
    /// `try_new` then adopts. If the trial wedges first, the dog resets into this bootloader,
    /// which reads the unconfirmed `Trial` and rolls back.
    pub fn start_for_trial(&mut self) {
        if self.handle.is_some() {
            return;
        }
        // `idle` is `None` only in the foreign-config case, where nothing can be started or fed.
        let Some(wdt0) = self.idle.take() else { return };
        if let Ok((_wdt, [handle])) = wdt::Watchdog::try_new::<_, 1>(wdt0, app_wdt_config()) {
            #[cfg(feature = "rtt")]
            defmt::info!("obc-boot: WDT started for the trial boot (24 s)");
            self.handle = Some(handle);
        }
        // The Err arm is unreachable: `take` saw the dog idle and nothing else here starts it.
    }
}
