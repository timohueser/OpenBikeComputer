//! The **named residual** of the legacy protocol — one list, every typed executor (#1397 S6).
//!
//! A typed executor drives [`App::run_pass`](crate::App::run_pass) and performs the plan's bounded
//! effects. What it still drains from the old [`HostMailbox`](crate::HostMailbox) is exactly three
//! commands, and the reason is the same on every platform: their domains cannot validate an
//! operation token, so they cannot own an outcome (epic #1433 §4.3).
//!
//! | Command | Why it is still here | Retires in |
//! |---|---|---|
//! | `FinishTrack` | Recorder has no machine — the close is answered by a catalog re-feed, not a ride identity (#1398) | #1398 |
//! | `ForgetBond` | The removal is confirmed by a link-status fact, not by a reply (#1400) | #1398/#1400 |
//! | `DeleteTrip` | `CatalogState::admit_intent` **refuses** a trip cascade: the bounded member read does not exist yet (#1491) | #1491 |
//!
//! This module is the list as data. It lives in `obc-app` rather than beside one executor because
//! there are two of them — `obc-host-core`'s [`HostLoop`] and the board's ride loop — and a residual
//! that drifted apart on the two would make S6c's deletion a per-host argument instead of a
//! compiler-verified sweep.
//!
//! [`HostLoop`]: https://docs.rs/obc-host-core
//!
//! `no_std` and `defmt`-free by construction: the board reports a violation through its own
//! transport, the hosts through [`assert_residual`].

use crate::host::HostCommandClass;
use crate::HostCommand;

/// The three residual classes in the **drain's** own vocabulary — what
/// [`App::drain_residual_commands`](crate::App::drain_residual_commands) asks for by name, and what
/// [`App::has_pending_residual_command`](crate::App::has_pending_residual_command) peeks at.
///
/// Asking by class rather than filtering a full drain is not a tidiness choice. For every class
/// DeviceCore owns, the full walk *pulls* from the domain — it mints the operation as it passes —
/// so an executor that walked it would consume the rider's request and then decline to perform it.
/// [`RESIDUAL`] is the same three classes as prose and [`residual`] as a predicate;
/// `the_residual_table_names_exactly_what_the_predicate_admits` pins all three together.
pub(crate) const RESIDUAL_CLASSES: [HostCommandClass; RESIDUAL_CLASS_COUNT] =
    [HostCommandClass::FinishTrack, HostCommandClass::ForgetBond, HostCommandClass::DeleteTrip];

/// How many residual classes there are — the [`HostMailbox`](crate::HostMailbox) capacity at which
/// one [`drain_residual_commands`](crate::App::drain_residual_commands) call always completes.
pub const RESIDUAL_CLASS_COUNT: usize = 3;

/// The legacy classes a typed executor still drains, and nothing else.
///
/// Prose for [`assert_residual`]'s message; [`residual`] is what actually decides, and
/// `the_residual_table_names_exactly_what_the_predicate_admits` pins the two together.
pub const RESIDUAL: [&str; 3] = ["FinishTrack", "ForgetBond", "DeleteTrip"];

/// Whether `command` is one of the three a typed executor deliberately leaves on the old protocol.
///
/// Anything else in the mailbox is a class DeviceCore already owns, and running it beside the effect
/// that carries it would perform the same work twice.
pub fn residual(command: &HostCommand) -> bool {
    matches!(command, HostCommand::FinishTrack(_) | HostCommand::ForgetBond | HostCommand::DeleteTrip { .. })
}

/// The production assertion behind [`RESIDUAL`]: a class DeviceCore owns must never come back on the
/// old protocol, because executing it beside the effect that carries it would plan, install or
/// delete twice — and a class that quietly reappeared would be the migration coming undone.
///
/// Panicking is right for a host executor. The board cannot panic over a stray command mid-ride, so
/// it checks [`residual`] itself and reports instead — see `ride.rs`.
pub fn assert_residual(command: &HostCommand) {
    assert!(
        residual(command),
        "{command:?} is DeviceCore's now — running it here would repeat the effect that carries it \
         (the residual is {RESIDUAL:?})"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackAction;

    /// [`RESIDUAL`] is the prose an [`assert_residual`] failure prints and [`residual`] is what
    /// actually decides — so they have to name the same three classes. A class added to one and not
    /// the other would either fail with a message that lists the wrong residual, or quietly widen
    /// the residual without anyone reading the list noticing.
    #[test]
    fn the_residual_table_names_exactly_what_the_predicate_admits() {
        let admitted = [
            ("FinishTrack", HostCommand::FinishTrack(TrackAction::Save)),
            ("FinishTrack", HostCommand::FinishTrack(TrackAction::Discard)),
            ("ForgetBond", HostCommand::ForgetBond),
            ("DeleteTrip", HostCommand::DeleteTrip { id: 7 }),
        ];
        for (name, command) in admitted {
            assert!(residual(&command), "{command:?} is in the residual and the predicate refuses it");
            assert!(RESIDUAL.contains(&name), "{name} is admitted but not in the printed table");
        }
        assert_eq!(RESIDUAL.len(), 3, "three classes, and the table says which");
        assert_eq!(
            RESIDUAL_CLASSES.len(),
            RESIDUAL.len(),
            "the class list the drain asks for is the same residual the predicate admits"
        );
        // Both directions, because either one alone lets a class be *substituted*: the length check
        // above only catches a shortened list, and a one-way containment check passes just as
        // happily when `DeleteTrip` is swapped for something the predicate never admits.
        for class in RESIDUAL_CLASSES {
            assert!(
                admitted.iter().any(|(_, c)| c.class() == class),
                "{class:?} is asked for by name but is not one of the admitted commands"
            );
        }
        for (_, command) in &admitted {
            assert!(
                RESIDUAL_CLASSES.contains(&command.class()),
                "{command:?} is admitted by the predicate but the drain never asks for its class"
            );
        }
    }
}
