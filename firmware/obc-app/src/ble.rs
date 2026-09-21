//! The host-to-app BLE event and state seam: the small app-vocabulary snapshot the host feeds in
//! each pass, plus the store-change signal the object store raises on a commit or delete.
//!
//! `obc-app` stays oblivious to the radio, and no `obc-ble` or board type crosses this boundary.
//! The host distils its link into a [`BleStatus`] and pushes it through
//! [`App::set_ble_status`](crate::App::set_ble_status). The app's own consumers read only these
//! app-side types.

/// The radio's link phase in app vocabulary, as the Bluetooth settings screen's status line shows
/// it. Three states, deliberately coarser than the board's own: the UI never needs "stack coming
/// up", which reads as [`Advertising`](BleLink::Advertising).
///
/// The connected indicator keys on [`Connected`](BleLink::Connected) only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BleLink {
    /// The radio is disabled: nothing advertises, nothing connects.
    Off,
    /// Powered and unconnected: advertising and connectable. The steady state, and the boot default
    /// until the host feeds the first real snapshot.
    #[default]
    Advertising,
    /// A central holds the single link.
    Connected,
}

/// The whole of what `obc-app` knows about the BLE link. Distilled by the host from its radio state
/// and fed in each pass. Deliberately tiny: everything the UI needs, nothing about descriptors,
/// MTUs or peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BleStatus {
    pub link: BleLink,
    /// The 6-digit LESC passkey to show while pairing, or `None` otherwise.
    /// [`App::set_ble_status`](crate::App::set_ble_status) opens a
    /// [`PasskeyScreen`](crate::screen::PasskeyScreen) when this goes `Some` and closes it when it
    /// clears.
    pub passkey: Option<u32>,
    /// The platform's paired state. It is not a durable deletion receipt.
    pub paired: bool,
}

impl BleStatus {
    /// The powered-but-unlinked default: advertising, no passkey, no bond. The app's boot value
    /// until the host feeds the first real snapshot.
    pub const DISCONNECTED: BleStatus = BleStatus { link: BleLink::Advertising, passkey: None, paired: false };

    /// Whether a central holds the link: the connected indicator's one question.
    pub fn connected(&self) -> bool {
        self.link == BleLink::Connected
    }
}

// The bond domain protocol. Forgetting a phone is one bounded platform operation, but its
// user-visible lifecycle is DeviceCore's: the confirm hold, the "not paired" row afterwards, and
// the fact that the link dropping is a separate external fact rather than part of this answer.

use crate::device_core::{BondTag, OperationToken};

/// What the rider asks of the bond store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondIntent {
    /// Forget the paired phone: the guarded hold on the Bluetooth screen.
    ForgetRequested,
}

/// The one bounded bond operation, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondEffect {
    /// Clear the bond store and drop the bonded connection.
    Forget { token: OperationToken<BondTag> },
}

impl BondEffect {
    pub fn token(&self) -> OperationToken<BondTag> {
        match self {
            BondEffect::Forget { token } => *token,
        }
    }
}

/// The controller resolving list is separate from durable and host encryption keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerClearance {
    Confirmed,
    /// Host removal queued a controller update without an acknowledgment.
    Unconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondError {
    /// No durable clear was acknowledged; host keys were not touched.
    StoreWriteFailed,
    /// The written slot could not be read back as empty; host keys were not touched.
    StoreVerifyFailed,
    /// Durable keys were cleared, but host key removal failed.
    HostKeysRemoveFailed,
    /// No physical work was admitted.
    QueueFull,
    Unsupported,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondOutcome {
    KeysRemoved {
        token: OperationToken<BondTag>,
        controller: ControllerClearance,
    },
    Failed {
        token: OperationToken<BondTag>,
        error: BondError,
    },
    /// The executor stopped before physical work started.
    Cancelled {
        token: OperationToken<BondTag>,
    },
}

impl BondOutcome {
    pub fn from_result(token: OperationToken<BondTag>, result: Result<ControllerClearance, BondError>) -> Self {
        match result {
            Ok(controller) => Self::KeysRemoved { token, controller },
            Err(error) => Self::Failed { token, error },
        }
    }

    pub fn token(&self) -> OperationToken<BondTag> {
        match self {
            Self::KeysRemoved { token, .. } | Self::Failed { token, .. } | Self::Cancelled { token } => *token,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondStatus {
    Idle,
    Pending,
    Failed(BondError),
    RestartRequired,
    Removed,
}

impl BondStatus {
    pub fn can_forget(self, paired: bool) -> bool {
        (paired || matches!(self, Self::Failed(_))) && !matches!(self, Self::Pending | Self::RestartRequired)
    }
}

/// One admitted removal. Link facts never resolve this operation.
pub(crate) struct BondMachine {
    tokens: crate::device_core::TokenSource<BondTag>,
    pending: Option<BondEffect>,
    status: BondStatus,
}

impl BondMachine {
    pub const fn new() -> Self {
        Self { tokens: crate::device_core::TokenSource::new(), pending: None, status: BondStatus::Idle }
    }

    pub fn request(&mut self, supported: bool, paired: bool) -> bool {
        if !supported || !self.status.can_forget(paired) {
            return false;
        }
        self.pending = Some(BondEffect::Forget { token: self.tokens.issue() });
        self.status = BondStatus::Pending;
        true
    }

    pub fn next_effect(&mut self) -> Option<BondEffect> {
        self.pending.take()
    }
    pub fn status(&self) -> BondStatus {
        self.status
    }

    pub fn apply_outcome(&mut self, outcome: BondOutcome) -> bool {
        if self.status != BondStatus::Pending || !self.tokens.is_current(outcome.token()) {
            return false;
        }
        self.tokens.invalidate();
        self.pending = None;
        self.status = match outcome {
            BondOutcome::KeysRemoved { controller: ControllerClearance::Confirmed, .. } => BondStatus::Removed,
            BondOutcome::KeysRemoved { controller: ControllerClearance::Unconfirmed, .. } => {
                BondStatus::RestartRequired
            }
            BondOutcome::Failed { error, .. } => BondStatus::Failed(error),
            BondOutcome::Cancelled { .. } => BondStatus::Failed(BondError::Cancelled),
        };
        true
    }
}

const _: () = assert!(core::mem::size_of::<BondEffect>() <= 4);
const _: () = assert!(core::mem::size_of::<BondOutcome>() <= 8);

/// A single platform request retained through radio phase changes and until its result is read.
/// Wake signals do not own the request or its result.
pub struct BondDelivery {
    queued: Option<BondEffect>,
    running: Option<OperationToken<BondTag>>,
    outcome: Option<BondOutcome>,
}

impl Default for BondDelivery {
    fn default() -> Self {
        Self::new()
    }
}

impl BondDelivery {
    pub const fn new() -> Self {
        Self { queued: None, running: None, outcome: None }
    }

    pub fn submit(&mut self, effect: BondEffect) -> Result<(), BondError> {
        if self.queued.is_some() || self.running.is_some() || self.outcome.is_some() {
            return Err(BondError::QueueFull);
        }
        self.queued = Some(effect);
        Ok(())
    }

    pub fn begin(&mut self) -> Option<BondEffect> {
        let effect = self.queued.take()?;
        self.running = Some(effect.token());
        Some(effect)
    }

    pub fn finish(&mut self, outcome: BondOutcome) -> bool {
        if self.running != Some(outcome.token()) {
            return false;
        }
        self.running = None;
        self.outcome = Some(outcome);
        true
    }

    pub fn take_outcome(&mut self) -> Option<BondOutcome> {
        self.outcome.take()
    }
}

#[cfg(test)]
mod bond_tests {
    use super::*;

    #[test]
    fn admission_and_each_terminal_failure_are_one_shot() {
        let mut bond = BondMachine::new();
        assert!(!bond.request(false, true));
        assert!(!bond.request(true, false));
        for error in [
            BondError::StoreWriteFailed,
            BondError::StoreVerifyFailed,
            BondError::HostKeysRemoveFailed,
            BondError::QueueFull,
            BondError::Unsupported,
        ] {
            assert!(bond.request(true, true));
            let effect = bond.next_effect().unwrap();
            assert!(!bond.request(true, true));
            assert!(bond.next_effect().is_none());
            let failed = BondOutcome::Failed { token: effect.token(), error };
            assert!(bond.apply_outcome(failed));
            assert!(!bond.apply_outcome(failed));
            assert_eq!(bond.status(), BondStatus::Failed(error));
            assert!(bond.next_effect().is_none());
        }
        assert!(bond.request(true, false), "a failure permits retry even when the link says unpaired");
    }

    #[test]
    fn delivery_retains_the_exact_result_until_consumed_and_refuses_overwrite() {
        let mut tokens = crate::device_core::TokenSource::new();
        let first = BondEffect::Forget { token: tokens.issue() };
        let second = BondEffect::Forget { token: tokens.issue() };
        let mut delivery = BondDelivery::new();
        delivery.submit(first).unwrap();
        assert_eq!(delivery.submit(second), Err(BondError::QueueFull));
        assert_eq!(delivery.begin(), Some(first));
        assert_eq!(delivery.submit(second), Err(BondError::QueueFull));
        assert!(delivery.begin().is_none());
        assert!(!delivery.finish(BondOutcome::Cancelled { token: second.token() }));
        let done = BondOutcome::KeysRemoved { token: first.token(), controller: ControllerClearance::Unconfirmed };
        assert!(delivery.finish(done));
        assert!(!delivery.finish(done));
        assert_eq!(delivery.submit(second), Err(BondError::QueueFull));
        assert_eq!(delivery.take_outcome(), Some(done));
        assert!(delivery.take_outcome().is_none());
        delivery.submit(second).unwrap();
        assert_eq!(delivery.begin(), Some(second));
    }
}
