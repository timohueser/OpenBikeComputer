//! The one seam between the engine and whatever holds the bytes.
//!
//! [`Engine`](super::Engine) never touches storage: it hands out a [`Command`] and takes back an
//! [`Outcome`]. Everything that can execute one of those commands — the in-memory
//! [`FakeTransaction`](crate::harness::FakeTransaction) of the harness, the kernel-backed
//! transaction `obc-storage` composes over the OBC2 store, and whatever the board wires in — is one
//! implementation of this trait, and the driver loop is written once against it.
//!
//! Keeping the trait *here* is what keeps the dependency direction honest. `obc-link` is a
//! transport-free, storage-free codec; it must never learn what a card is. So the seam is declared
//! by the crate that needs it and implemented by the crate that owns the medium, exactly as
//! `obc-storage -> obc-link` already runs for the identity types.
//!
//! The trait is deliberately one method. A transaction that could be asked for more than "execute
//! this command" would let a caller reach around the engine's state machines, and the whole point
//! of the effect seam is that there is nothing to reach around: the command names the step, the
//! outcome names what happened, and the ordering between them is the engine's alone.

use super::effect::{Command, Outcome};

/// Something that can execute the engine's typed commands.
///
/// `scratch` is the caller's buffer for any bytes an outcome hands back — source bytes for a
/// download frame, an echo payload. It is borrowed rather than owned so an implementation on the
/// board writes straight into the record buffer the adapter is about to send, and so this trait
/// stays allocation-free.
pub trait Transaction {
    /// Executes one command and reports its outcome.
    ///
    /// A total function: every failure is an [`Outcome::Failed`] carrying the §12 cause, never a
    /// panic and never an error type of the implementation's own. The engine has one unwind path,
    /// and it is driven by that cause.
    fn execute<'s>(&mut self, command: Command<'_>, scratch: &'s mut [u8]) -> Outcome<'s>;
}

impl<T: Transaction + ?Sized> Transaction for &mut T {
    fn execute<'s>(&mut self, command: Command<'_>, scratch: &'s mut [u8]) -> Outcome<'s> {
        (**self).execute(command, scratch)
    }
}
