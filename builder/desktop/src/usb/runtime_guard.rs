//! A test that the cargo feature which makes `nusb` usable from an async context is still on.
//!
//! `nusb` returns [`MaybeFuture`](nusb::MaybeFuture) rather than a plain future, so the same call
//! serves a blocking caller and an async one. The async arm needs somewhere to put the blocking
//! syscall, and which executor that is comes from the `smol` or `tokio` cargo feature. With neither
//! enabled, `BlockingTask::spawn` panics. That is not a compile error: the crate builds, clippy is
//! happy, and every `.await` on a `nusb` future panics the first time a real device is touched.
//!
//! What enforces it: Windows builds `list_devices()` from a blocking arm, so awaiting it reaches
//! the panic with no device attached, and the Windows CI leg fails if the feature is dropped. Linux
//! and macOS build the same call from a ready arm, so only the second half of this module covers
//! them, and that half runs only where a device is plugged in.

#[cfg(test)]
mod tests {
    /// Await a `nusb` `MaybeFuture` and require it not to panic.
    ///
    /// `list_devices()` is the only entry point that needs no hardware, so it is the one a CI
    /// machine can run. The assertion is about reaching the result and not about its contents: a
    /// machine with no USB devices is a legitimate pass.
    #[tokio::test]
    async fn awaiting_nusb_does_not_panic() {
        let devices = nusb::list_devices().await.expect("the USB bus should be enumerable");
        // Draining it proves this is a real iterator and not an unpolled future.
        let count = devices.count();
        println!("nusb enumerated {count} device(s)");
    }

    /// The half that covers macOS and Linux, on any machine with the device plugged in.
    ///
    /// Opening is blocking on all three platforms, so this reaches the panicking arm everywhere. It
    /// skips loudly when the device is absent, because a silent skip would read as coverage that
    /// does not exist.
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
