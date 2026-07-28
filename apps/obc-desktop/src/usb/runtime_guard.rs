//! The one thing about `nusb` that a compile cannot tell you.
//!
//! `nusb` returns [`MaybeFuture`](nusb::MaybeFuture) rather than a plain future, so the same call
//! serves a blocking caller (`.wait()`) and an async one (`.await`). The async arm needs somewhere
//! to put the blocking syscall, and which executor that is comes from a **cargo feature** —
//! `smol` or `tokio`. With neither enabled, `BlockingTask::spawn` is
//!
//! ```text
//! panic!("Awaiting blocking syscall without an async runtime: enable the `smol` or `tokio` feature of nusb.")
//! ```
//!
//! That is not a compile error, a deprecation or a warning. The crate builds, `clippy` is happy,
//! and every `.await` in [`super::link`] and [`super::watch`] panics the first time a real device
//! is touched — on a tokio worker, so the Tauri command's caller sees a rejected promise and the
//! window simply shows nothing. It shipped exactly that way in #909 and survived a green three-OS
//! CI matrix, because the matrix *compiles* this crate and every USB test stands on a mock at the
//! Tauri command boundary. Nothing in CI had ever awaited a `nusb` future.
//!
//! Hence this module. It is not a test of `nusb`; it is a test that the feature which makes `nusb`
//! usable from an async context is still switched on.
//!
//! ## What actually enforces it
//!
//! Honest accounting, because a guard nobody can see fail is not a guard:
//!
//! - **Windows** builds `list_devices()` from `Blocking::new(…)`, so awaiting it hits the panicking
//!   arm **with no device attached**. The `desktop (windows-latest, …)` CI leg is what fails if the
//!   feature is dropped.
//! - **Linux and macOS** build the same call from `Ready(…)`, which never reaches
//!   `BlockingTask::spawn`. On those two the device-free half of this test passes either way, and
//!   what covers them is the second half — which runs only where a device is plugged in, i.e. on a
//!   developer's machine, never in CI.
//!
//! So: one CI leg and every developer with hardware. If `nusb` ever makes enumeration blocking on
//! the other platforms, this tightens by itself.

#[cfg(test)]
mod tests {
    /// Await a `nusb` `MaybeFuture` and require it not to panic.
    ///
    /// `list_devices()` is the only entry point that needs no hardware, so it is the one a CI
    /// machine can run. The assertion is deliberately about *reaching* the result rather than about
    /// its contents: a machine with no USB devices at all is a legitimate pass, and a bus that
    /// cannot be enumerated is a real error worth reporting rather than a panic worth hiding.
    #[tokio::test]
    async fn awaiting_nusb_does_not_panic() {
        let devices = nusb::list_devices().await.expect("the USB bus should be enumerable");
        // Draining it proves we own a real iterator, not an unpolled future.
        let count = devices.count();
        println!("nusb enumerated {count} device(s)");
    }

    /// The half that covers macOS and Linux, on any machine with the device plugged in.
    ///
    /// Opening is `Blocking` on all three platforms, so this reaches the panicking arm everywhere —
    /// it just cannot run on a CI box with no hardware. Skipped, loudly, when the device is absent;
    /// a silent skip would read as coverage that does not exist.
    #[tokio::test]
    async fn opening_the_device_does_not_panic() {
        let Some(info) = nusb::list_devices()
            .await
            .expect("the USB bus should be enumerable")
            .find(|d| d.vendor_id() == super::super::VENDOR_ID && d.product_id() == super::super::PRODUCT_ID)
        else {
            println!("no OpenBikeComputer attached — skipping (this half needs hardware)");
            return;
        };
        // The failure this guards is a panic, not an `Err`. A device held open by the app is an
        // ordinary `Err` and must not fail the test.
        match info.open().await {
            Ok(_) => println!("opened the device"),
            Err(e) => println!("device present but not openable ({e}) — the await itself is what mattered"),
        }
    }
}
