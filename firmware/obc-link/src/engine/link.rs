//! The ByteLink seam: the only thing the engine knows about a physical link.
//!
//! §14 draws the line and this trait is it: "The common frame bytes above are identical on both
//! links. Adapters own only authentication facts, record boundaries, pacing, timeout, and drain
//! completion." So a `ByteLink` moves whole records, reports its own ceilings, and names the
//! identity that scopes a session. It has no notion of an object kind, a CRC, staging, or progress,
//! and nothing in the engine may add one.
//!
//! BLE implements it with the `objectControl` characteristic and an L2CAP CoC; USB implements it
//! with two bulk endpoint pairs and `record_length` framing. The fake links in the test harness
//! implement it with the same two shapes, which is what lets one transcript run on both.

use super::connection::LinkCeilings;
use super::session::LinkContext;

/// Which of a binding's two record channels a record belongs to (§14.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkChannel {
    /// One §2 control frame per record, in strict request/response order.
    Control,
    /// One §13 stream frame per record.
    Stream,
}

/// Why a link operation could not complete. These are physical facts, not protocol errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// The peer went away. The coordinator is told once, with the exact context.
    Disconnected,
    /// The record could not be delivered within the adapter's bound — a BLE indication that was
    /// never confirmed, or a USB transfer that never completed.
    Timeout,
    /// The record is larger than this channel can carry.
    RecordTooLarge,
    /// The transport itself failed.
    TransportFault,
}

/// One physical link, as the engine's driver sees it.
pub trait ByteLink {
    /// The identity that scopes every session issued on this connection.
    fn context(&self) -> LinkContext;

    /// The record ceilings this binding's framing imposes (§14.0).
    fn ceilings(&self) -> LinkCeilings;

    /// Receives one whole record into `buffer`, returning its length.
    ///
    /// `Ok(None)` means nothing is waiting. A record never spans two calls: §14.1 gives BLE one
    /// frame per ATT value and one per CoC SDU, and §14.2 gives USB one frame per length-prefixed
    /// record.
    fn receive(&mut self, channel: LinkChannel, buffer: &mut [u8]) -> Result<Option<usize>, LinkError>;

    /// Sends one whole record.
    fn send(&mut self, channel: LinkChannel, record: &[u8]) -> Result<(), LinkError>;

    /// Completes every accepted outbound record, or reports the bounded drain timeout.
    ///
    /// §14.1 and §14.2 both make this the last thing before an update reboot: "Completion is
    /// transport drain, not proof that the host application persisted the response."
    fn drain(&mut self) -> Result<(), LinkError>;

    /// Closes one record channel, which §2 and §13 require for untrusted framing.
    fn close(&mut self, channel: LinkChannel);
}
