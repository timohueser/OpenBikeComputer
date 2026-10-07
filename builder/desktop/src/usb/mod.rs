//! Native USB: `nusb` under the shared byte-pipe seam.
//!
//! Tauri's webview has no WebUSB, so the desktop app drives USB itself. This is the only universal
//! USB path, and it is what a Safari or Firefox user installs the app for.
//!
//! The protocol is not reimplemented here. The object model, the descriptor codecs, the
//! whole-object CRC and the client live once, in TypeScript, over a pluggable `BytePipe`, so a
//! second host drops a transport underneath them instead of growing a parallel implementation that
//! drifts from a byte-exact wire contract. This module moves bytes and nothing else: every `0x` in
//! the non-test code is USB's own vocabulary, a vendor id, a product id, an interface class or an
//! endpoint-address direction bit.
//!
//! The control plane carries the protocol's control records and the stream plane carries its stream
//! records, both over IPC.
//!
//! There is no path that streams a file straight from disk into the endpoint. A record stream is
//! framed, and a raw byte streamer cannot produce one without a second implementation of the wire
//! contract. If a measurement ever says the IPC boundary is the bottleneck, the honest shape is a
//! Rust-side record sender that shares the codec.
//!
//! Both IPC directions carry raw binary and not JSON: a command result is [`tauri::ipc::Response`]
//! and a write arrives as [`tauri::ipc::InvokeBody::Raw`] with its handle and plane in headers. A
//! `Vec<u8>` argument would be serialised as a JSON array of numbers, about four bytes of text per
//! byte of payload.
//!
//! The window has no filesystem capability. A file the rider picks arrives through the webview as a
//! `File` with no path at all, exactly as it does on the web.

pub mod runtime_guard;
pub mod watch;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeBody, Request, Response};
use tauri::State;

use obc_usb::{Dir, OpenLink, OpenedLink, PipeFault, Plane};
use watch::{DeviceSummary, UsbEvent};

pub use obc_usb::{PRODUCT_ID, VENDOR_ID};

/// Every device this app has open, plus the one hot-plug watch behind them.
#[derive(Default)]
pub struct UsbState {
    links: Mutex<HashMap<u32, Arc<OpenLink>>>,
    next_handle: AtomicU32,
    /// Where the watch task sends events. Replaceable, so a window that reloaded re-points the
    /// existing watch at its new channel instead of starting a second one.
    sink: Mutex<Option<Channel<UsbEvent>>>,
    watching: Mutex<bool>,
}

impl UsbState {
    fn link(&self, handle: u32) -> Result<Arc<OpenLink>, PipeFault> {
        self.links
            .lock()
            .expect("usb links")
            .get(&handle)
            .cloned()
            .ok_or_else(|| PipeFault::closed("The device link is closed."))
    }

    /// Fail and forget every link on a device that has gone away.
    fn disconnected(&self, device_id: &str) {
        let mut links = self.links.lock().expect("usb links");
        links.retain(|_, link| {
            if link.device_id == device_id {
                link.cancel_all();
                false
            } else {
                true
            }
        });
    }

    fn drop_all(&self) {
        for (_, link) in self.links.lock().expect("usb links").drain() {
            link.cancel_all();
        }
    }
}

/// Start or re-point the hot-plug watch and report what is attached now.
///
/// It answers the matching devices after the watch is live, which is the order that cannot miss a
/// device plugged in during the call.
///
/// Any link left over from a previous page load is dropped first. A reloaded window has no handles
/// but the backend still holds the interface claim, and a stale claim is what makes the next
/// `usb_open` fail as busy.
#[tauri::command]
pub async fn usb_watch(
    state: State<'_, Arc<UsbState>>,
    on_event: Channel<UsbEvent>,
) -> Result<Vec<DeviceSummary>, String> {
    let state = Arc::clone(&state);
    state.drop_all();
    *state.sink.lock().expect("usb sink") = Some(on_event);

    {
        let mut watching = state.watching.lock().expect("usb watching");
        if !*watching {
            *watching = true;
            let for_events = Arc::clone(&state);
            let for_disconnect = Arc::clone(&state);
            // One long-lived watch serves a window that reloads, because the task resolves the
            // sink per event rather than capturing the channel it was born with.
            watch::spawn(
                Arc::new(move |event: UsbEvent| {
                    if let Some(channel) = for_events.sink.lock().expect("usb sink").as_ref() {
                        let _ = channel.send(event);
                    }
                }),
                Arc::new(move |device_id: &str| for_disconnect.disconnected(device_id)),
            );
        }
    }

    watch::list().await
}

/// The matching devices attached right now, without touching the watch. A native host has no
/// chooser, so asking for a device is looking again.
#[tauri::command]
pub async fn usb_list() -> Result<Vec<DeviceSummary>, String> {
    watch::list().await
}

/// Open, configure and claim a device; hand back a handle and the two pipes' packet sizes.
#[tauri::command]
pub async fn usb_open(state: State<'_, Arc<UsbState>>, device_id: String) -> Result<OpenedLink, PipeFault> {
    let devices =
        nusb::list_devices().await.map_err(|e| PipeFault::device(format!("USB devices could not be listed: {e}")))?;
    let info = devices
        .filter(watch::matches)
        .find(|info| watch::device_key(info.id()) == device_id)
        .ok_or_else(|| PipeFault::closed("That device is no longer attached."))?;

    let (link, mut opened) = obc_usb::open(&info, device_id).await?;
    let handle = state.next_handle.fetch_add(1, Ordering::Relaxed) + 1;
    opened.handle = handle;
    state.links.lock().expect("usb links").insert(handle, link);
    Ok(opened)
}

/// Release a link. Idempotent; everything parked on it settles first.
#[tauri::command]
pub fn usb_close(state: State<'_, Arc<UsbState>>, handle: u32) {
    let link = state.links.lock().expect("usb links").remove(&handle);
    // The cancel lets a pending read return instead of holding the interface claim until the
    // device says something. The claim drops with the last `Arc`, which the cancelled transfers
    // hold.
    if let Some(link) = link {
        link.cancel_all();
    }
}

/// One transfer's worth of bytes off a plane's IN endpoint.
///
/// A read is not a message on the bulk plane: it hands back whatever the transport delivered, and
/// the client accumulates to the length its descriptor announced.
#[tauri::command]
pub async fn usb_read(state: State<'_, Arc<UsbState>>, handle: u32, plane: Plane) -> Result<Response, PipeFault> {
    let link = state.link(handle)?;
    let bytes = link.plane(plane).read().await?;
    // `Response` is the raw path: the frontend gets an ArrayBuffer, not a JSON array of numbers.
    Ok(Response::new(bytes))
}

/// Write one transfer to a plane's OUT endpoint.
///
/// It takes the raw invoke body rather than a `Vec<u8>` argument. `handle` and `plane` ride in
/// headers, because a raw body is the whole payload.
#[tauri::command]
pub async fn usb_write(state: State<'_, Arc<UsbState>>, request: Request<'_>) -> Result<(), PipeFault> {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| PipeFault::device(format!("a usb_write is missing its `{name}` header.")))
    };
    let handle: u32 =
        header("handle")?.parse().map_err(|_| PipeFault::device("a usb_write carried an unreadable handle."))?;
    let plane = match header("plane")? {
        "control" => Plane::Control,
        "bulk" => Plane::Bulk,
        other => return Err(PipeFault::device(format!("a usb_write named an unknown plane `{other}`."))),
    };
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(PipeFault::device("a usb_write must carry its bytes as a raw body."));
    };
    let link = state.link(handle)?;
    link.plane(plane).write(bytes).await
}

/// Cancel whatever is parked on one direction of a plane, so the caller's `AbortSignal` reaches the
/// transport rather than only releasing the JavaScript promise.
#[tauri::command]
pub fn usb_cancel(state: State<'_, Arc<UsbState>>, handle: u32, plane: Plane, dir: Option<Dir>) {
    if let Ok(link) = state.link(handle) {
        link.plane(plane).cancel(dir);
    }
}

/// Return a plane to a known-empty state.
#[tauri::command]
pub async fn usb_reset(state: State<'_, Arc<UsbState>>, handle: u32, plane: Plane) -> Result<(), PipeFault> {
    state.link(handle)?.plane(plane).reset().await
}

#[cfg(test)]
mod tests {
    /// The policy without a Tauri app handle: the same canonicalise-then-prefix rule, over a real
    /// temp tree so the symlink case is real.
    fn allowed(root: &std::path::Path, candidate: &std::path::Path) -> bool {
        let (Ok(root), Ok(candidate)) = (std::fs::canonicalize(root), std::fs::canonicalize(candidate)) else {
            return false;
        };
        candidate.is_file() && candidate.starts_with(&root)
    }

    #[test]
    fn only_files_inside_an_app_owned_root_are_sendable() {
        let base = std::env::temp_dir().join(format!("obc-usb-policy-{}", std::process::id()));
        let root = base.join("maps");
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(root.join("ok.obcm"), b"x").expect("write");
        std::fs::write(outside.join("secret"), b"x").expect("write");

        assert!(allowed(&root, &root.join("ok.obcm")));
        assert!(!allowed(&root, &outside.join("secret")));
        // The traversal a prefix check alone would admit, and canonicalising does not.
        assert!(!allowed(&root, &root.join("../elsewhere/secret")));
        // A directory is not a file.
        assert!(!allowed(&root, &root));

        #[cfg(unix)]
        {
            // Why both sides are canonicalised: a symlink inside the root that points out of it
            // resolves to its target, and the target fails the prefix check.
            let link = root.join("escape");
            std::os::unix::fs::symlink(outside.join("secret"), &link).expect("symlink");
            assert!(!allowed(&root, &link));
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
