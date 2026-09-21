//! The same device, behind `wasm-bindgen`.
//!
//! Records in, one reaction out, exactly as the native surface: the JavaScript adapter owns the
//! record framing, the packet slicing and the backpressure, and calls back for the next reaction
//! when it has taken the last one. Nothing here loops, buffers a download or knows what USB is.
//!
//! Device information is not here. The loopback link on the TypeScript side already answers it, and
//! a second opinion about a constant would be a device policy this crate has no business holding.

use obc_storage::flat::{EntryFlags, StoreId};
use wasm_bindgen::prelude::*;

use crate::json::{catalog_json, trace_json};
use crate::sim::{SimDevice, SimOptions};
use crate::Reaction;
use obc_link::flat::Channel;

/// Bytes §4's rollback reserve asks for when a test allows `ARM`. The size is arbitrary — what the
/// tests care about is that one reserve is committed and that it holds extents.
const ARM_RESERVE: u64 = 1024 * 1024;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// What the device wants done next.
///
/// `kind` is one of `idle`, `send`, `close`, `send-and-reboot`; `channel` is `control` or `stream`
/// and is meaningless for `idle`.
#[wasm_bindgen(js_name = DeviceReaction)]
pub struct JsReaction {
    kind: &'static str,
    channel: &'static str,
    bytes: Vec<u8>,
}

#[wasm_bindgen(js_class = DeviceReaction)]
impl JsReaction {
    #[wasm_bindgen(getter)]
    pub fn kind(&self) -> String {
        self.kind.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn channel(&self) -> String {
        self.channel.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

impl From<Reaction> for JsReaction {
    fn from(reaction: Reaction) -> JsReaction {
        let name = |channel: Channel| match channel {
            Channel::Control => "control",
            Channel::Stream => "stream",
        };
        match reaction {
            Reaction::Idle => JsReaction { kind: "idle", channel: "control", bytes: Vec::new() },
            Reaction::Send { channel, bytes } => JsReaction { kind: "send", channel: name(channel), bytes },
            Reaction::Close(channel) => JsReaction { kind: "close", channel: name(channel), bytes: Vec::new() },
            Reaction::SendAndReboot { bytes } => JsReaction { kind: "send-and-reboot", channel: "control", bytes },
        }
    }
}

/// The real engine, the real store and a real card, over the cable.
#[wasm_bindgen(js_name = FlatDevice)]
pub struct JsDevice {
    inner: SimDevice,
}

#[wasm_bindgen(js_class = FlatDevice)]
impl JsDevice {
    #[wasm_bindgen(constructor)]
    pub fn new(
        extents: u32,
        formatted: bool,
        seed: u32,
        control_ceiling: usize,
        stream_ceiling: usize,
        arm_allowed: bool,
        store_id: &str,
    ) -> JsDevice {
        JsDevice {
            inner: SimDevice::boot(SimOptions {
                extents,
                formatted,
                seed: seed as u64,
                store: store_id_of(store_id),
                control_ceiling,
                stream_ceiling,
                arm_reserve: arm_allowed.then_some(ARM_RESERVE),
                ..SimOptions::default()
            }),
        }
    }

    #[wasm_bindgen(js_name = onControl)]
    pub fn on_control(&mut self, record: &[u8]) -> JsReaction {
        self.inner.on_control(record).into()
    }

    #[wasm_bindgen(js_name = onStream)]
    pub fn on_stream(&mut self, record: &[u8]) -> JsReaction {
        self.inner.on_stream(record).into()
    }

    pub fn poll(&mut self) -> JsReaction {
        self.inner.poll().into()
    }

    pub fn reboot(&mut self) {
        self.inner.reboot();
    }

    #[wasm_bindgen(js_name = commitSequence)]
    pub fn commit_sequence(&self) -> u64 {
        self.inner.commit_sequence()
    }

    #[wasm_bindgen(js_name = storeId)]
    pub fn store_id(&self) -> String {
        self.inner.store_id().0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The catalog as JSON, in catalog order.
    pub fn catalog(&self) -> String {
        catalog_json(&self.inner.catalog())
    }

    /// The bytes behind one entry, or `undefined`. A revision of zero means the head.
    #[wasm_bindgen(js_name = readObject)]
    pub fn read_object(&self, id: u64, revision: u64) -> Option<Vec<u8>> {
        self.inner.read_object(id, revision)
    }

    /// Publish an object straight through the store seam. Returns the committed entry as JSON — the
    /// store assigns the id, so the caller reads it back rather than choosing it.
    pub fn seed(&mut self, kind: u16, name: &str, bytes: &[u8]) -> String {
        catalog_json(&[self.inner.seed(kind, bytes, name)])
    }

    /// Publish an entry over a reserve with no bytes behind it: a recording ride, or an update's
    /// rollback reserve.
    #[wasm_bindgen(js_name = seedReserved)]
    pub fn seed_reserved(&mut self, kind: u16, name: &str, reserve: u64, flags: u16) -> String {
        catalog_json(&[self.inner.seed_reserved(kind, reserve, EntryFlags::decode(flags).expect("§5.3 flags"), name)])
    }

    /// Publish a further revision of an object already on the card, keeping the previous one as
    /// `RETAINED`. The id is the card's, read off an earlier seed.
    #[wasm_bindgen(js_name = seedRetained)]
    pub fn seed_retained(&mut self, id: u64, name: &str, bytes: &[u8]) -> String {
        catalog_json(&[self.inner.seed_retained(id, bytes, name)])
    }

    #[wasm_bindgen(js_name = traceRequests)]
    pub fn trace_requests(&mut self) {
        self.inner.trace_requests();
    }

    /// Every control record served since the last call, as JSON.
    #[wasm_bindgen(js_name = takeTrace)]
    pub fn take_trace(&mut self) -> String {
        trace_json(&self.inner.take_request_trace())
    }

    #[wasm_bindgen(js_name = stopAnswering)]
    pub fn stop_answering(&mut self) {
        self.inner.stop_answering();
    }
}

/// §5.7's store identity, as 32 hex characters.
fn store_id_of(hex: &str) -> StoreId {
    let mut bytes = [0u8; 16];
    assert_eq!(hex.len(), 32, "a store identity is 32 hex characters");
    for (at, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).expect("a hex store identity");
    }
    StoreId(bytes)
}
