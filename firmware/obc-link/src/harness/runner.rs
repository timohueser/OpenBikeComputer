//! The driver loop: records in, commands to the transaction, records out.
//!
//! This is the whole of what the board glue has to do, written once and generic over the link. It
//! is deliberately small — the engine holds the state, the transaction holds the bytes, and the
//! driver only carries values between them — because "one upload and one download implementation
//! serve BLE and USB" is only true if the thing above them is this thin.

use crate::engine::{DeviceProfile, Engine, LinkChannel, LinkError, Reaction};
use crate::frame::MAX_STREAM_FRAME;

use super::fake_link::FakeLink;
use super::transaction::FakeTransaction;

/// One link, one engine, one transaction.
#[derive(Debug)]
pub struct Driver<L: FakeLink> {
    /// The physical link.
    pub link: L,
    /// The engine under test.
    pub engine: Engine,
    /// The transaction the engine's commands are executed against.
    pub transaction: FakeTransaction,
}

impl<L: FakeLink> Driver<L> {
    /// Builds a driver and opens the link's connection generation.
    pub fn new(link: L, profile: DeviceProfile, transaction: FakeTransaction) -> Self {
        let mut engine = Engine::new(profile);
        engine.open_connection(link.context(), link.ceilings());
        Driver { link, engine, transaction }
    }

    /// Reopens the connection — a reconnect, which makes every earlier SessionId stale (§3).
    pub fn reopen(&mut self, generation: u32) {
        self.link.set_generation(generation);
        self.engine.open_connection(self.link.context(), self.link.ceilings());
    }

    /// Reports link teardown to the coordinator exactly once, with the exact context (§13).
    pub fn close(&mut self) {
        let context = self.link.context();
        let reaction = self.engine.close_connection(context);
        let mut out = [0u8; MAX_STREAM_FRAME];
        let mut scratch = [0u8; MAX_STREAM_FRAME];
        let _ = Self::settle(&mut self.link, &mut self.engine, &mut self.transaction, reaction, &mut out, &mut scratch);
    }

    /// Drains every waiting record, then every download frame that is due.
    pub fn pump(&mut self) -> Result<(), LinkError> {
        let mut record = [0u8; MAX_STREAM_FRAME];
        let mut out = [0u8; MAX_STREAM_FRAME];
        let mut scratch = [0u8; MAX_STREAM_FRAME];
        loop {
            let mut progressed = false;
            for channel in [LinkChannel::Control, LinkChannel::Stream] {
                match self.link.receive(channel, &mut record) {
                    Ok(Some(len)) => {
                        progressed = true;
                        let context = self.link.context();
                        let reaction = match channel {
                            LinkChannel::Control => self.engine.on_control(context, &record[..len], &mut out),
                            LinkChannel::Stream => self.engine.on_stream(context, &record[..len], &mut out),
                        };
                        Self::settle(
                            &mut self.link,
                            &mut self.engine,
                            &mut self.transaction,
                            reaction,
                            &mut out,
                            &mut scratch,
                        )?;
                    }
                    Ok(None) => {}
                    Err(LinkError::TransportFault) => {
                        // §14.2: a malformed record length "resets only the affected USB record
                        // stream before session teardown is reported to the coordinator".
                        progressed = true;
                        self.link.close(channel);
                    }
                    Err(error) => return Err(error),
                }
            }
            loop {
                let reaction = self.engine.poll_download();
                if matches!(reaction, Reaction::Idle) {
                    break;
                }
                progressed = true;
                Self::settle(
                    &mut self.link,
                    &mut self.engine,
                    &mut self.transaction,
                    reaction,
                    &mut out,
                    &mut scratch,
                )?;
            }
            if !progressed {
                return Ok(());
            }
        }
    }

    /// Completes every accepted outbound record, or reports the bounded drain timeout (§14).
    pub fn drain(&mut self) -> Result<(), LinkError> {
        self.link.drain()
    }

    fn settle(
        link: &mut L,
        engine: &mut Engine,
        transaction: &mut FakeTransaction,
        mut reaction: Reaction<'_>,
        out: &mut [u8],
        scratch: &mut [u8],
    ) -> Result<(), LinkError> {
        loop {
            match reaction {
                Reaction::Idle => return Ok(()),
                Reaction::Emit { channel, len } => {
                    return match link.send(channel, &out[..len]) {
                        // A response the link never delivered is unknown delivery, not a failed
                        // mutation (§13): the device has already committed, so the driver carries
                        // on and the client's QueryOperation is what settles it.
                        Err(LinkError::Timeout) => Ok(()),
                        other => other,
                    };
                }
                Reaction::Close(channel) => {
                    link.close(channel);
                    return Ok(());
                }
                Reaction::Work(command) => {
                    let outcome = transaction.execute(command, scratch);
                    reaction = engine.resume(outcome, out);
                }
            }
        }
    }
}
