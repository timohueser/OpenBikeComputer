//! `obc-link` — the device's wire codecs and its one transfer engine.
//!
//! [`flat`] is the live one: **protocol v4**, the wire of the flat card store, frozen by
//! [`FLAT_Store_Protocol.md`] — the `OBC4` control frame, the eight opcodes, the stream frame, the
//! error body, the store seam the engine declares, and the transfer engine itself. Start there.
//!
//! [`FLAT_Store_Protocol.md`]: ../../../specs/FLAT_Store_Protocol.md
//!
//! Everything below this paragraph is the **superseded** Device Object System v2 wire (major 3) and
//! its engine, kept while the OBC2 consumers that still read it are migrated (epic #1256, FS11
//! deletes both). It is not extended and nothing in [`flat`] forwards to it.
//!
//! One Rust implementation of the bytes frozen by the normative suite in `specs/`:
//!
//! - [`Device_Object_Protocol_v3.md`] — the OBCP wire major 3 control frame, every opcode's request
//!   and response body, the stream frame and its fault body, the typed result envelope, the error
//!   body and its retry matrix, and the canonical-intent digest;
//! - [`Device_Object_Registries_v2.md`] — object and draft-part kinds, the per-kind metadata field
//!   tables, result outcomes, and the domain semantic detail registry;
//! - [`Device_Object_System_v2.md`] — the mechanically distinct identities in [`ids`].
//!
//! [`Device_Object_Protocol_v3.md`]: ../../../specs/Device_Object_Protocol_v3.md
//! [`Device_Object_Registries_v2.md`]: ../../../specs/Device_Object_Registries_v2.md
//! [`Device_Object_System_v2.md`]: ../../../specs/Device_Object_System_v2.md
//!
//! ## What this crate is, and what it deliberately is not
//!
//! It is a codec and nothing else. Decoding is **total** — every input is either a typed message or
//! a typed [`DecodeError`] carrying the spec's own category and detail — **bounded**, and
//! allocation-free: a variable-length body is borrowed from the caller's record buffer, never
//! copied into one this crate owns. Encoding produces exact bytes: fixed-size messages return
//! fixed-size arrays, variable ones write into a caller-provided slice and report the length.
//!
//! The codec proper contains no transport, no session state, no storage, and no policy. It does not
//! know what a BLE characteristic or a USB endpoint is, it never decides whether an operation is
//! authorized, and it holds no state between calls. Those are the adapter's and the engine's jobs;
//! this crate is what they agree on. That is also why it has exactly two dependencies — `obc-crc`
//! for the contract's CRC-32/IEEE and `sha2` for the canonical-intent digest.
//!
//! ## The engine
//!
//! [`engine`] is the device half *above* that codec: the connection state machine of §5.2, the
//! SessionId coordinator of §3, and the upload/download/command machines of §15. It is still
//! transport-free and storage-free — a record goes in and either bytes or a typed command comes out,
//! and the board glue executes the command against the DOS2 transaction seam — which is what lets
//! one engine serve BLE and USB and be proved without a radio or a card. [`harness`] is the host-only
//! side of that proof: the `ByteLink` seam implemented twice, an in-memory transaction with the DOS2
//! lifecycle's shape, and the checked-in semantic transcripts replayed through both.
//!
//! ## Reading order
//!
//! [`frame`] is the envelope every control message arrives in; [`request`] and [`response`] are the
//! two dispatching enums that turn a decoded frame into a typed message. Everything else is the
//! parts they are built from: [`ids`], [`registry`], [`metadata`], [`result`], [`error`],
//! [`stream`], and [`intent`]. [`engine`] sits on top of all of them.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(any(test, feature = "std"))]
extern crate std;

mod codec;

pub mod control;
pub mod download;
pub mod draft;
pub mod engine;
pub mod error;
pub mod flat;
pub mod frame;
pub mod hello;
pub mod ids;
pub mod intent;
pub mod metadata;
pub mod mutate;
pub mod query;
pub mod registry;
pub mod request;
pub mod response;
pub mod result;
pub mod stream;
pub mod upload;

#[cfg(any(test, feature = "std"))]
pub mod harness;

#[cfg(any(test, feature = "std"))]
pub mod vectors;

pub use error::{DecodeError, ErrorBody, ErrorCategory, Owner, RetryGuidance};
pub use frame::{ControlFrame, FrameFlags, Opcode, MAX_CONTROL_FRAME, MIN_CONTROL_FRAME};
pub use request::Request;
pub use response::Response;

/// The wire major this crate implements. A frame carrying any other value is
/// `incompatibleVersion/unsupportedMajor` (`Device_Object_Protocol_v3.md` §2).
pub const WIRE_MAJOR: u8 = 3;

/// The wire minor this crate implements. Frames are minor `0` in v3.0; the *device's* minor is
/// learned from Capabilities byte 55, never from a frame header (§2.1, §5).
pub const WIRE_MINOR: u8 = 0;

/// The OBC2 storage-format version this wire major pairs with (Capabilities byte 1).
pub const STORAGE_FORMAT_VERSION: u8 = 1;

/// Result alias for the crate's total decoders.
pub type Result<T> = core::result::Result<T, DecodeError>;

/// Encoding failed because the caller's output slice is too small for the exact bytes.
///
/// This is the only way an encode can fail: every other property of an encodable message is
/// established when the message is constructed, so there is nothing else to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferTooSmall {
    /// The number of bytes the encoding needs.
    pub needed: usize,
    /// The number of bytes the caller offered.
    pub available: usize,
}

/// Result alias for encoders that write into a caller-provided slice.
pub type EncodeResult = core::result::Result<usize, BufferTooSmall>;
