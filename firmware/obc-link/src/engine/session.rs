//! Session issue, revocation, and ownership (`Device_Object_Protocol_v3.md` §3 and §13).
//!
//! §3 gives the coordinator four obligations and this module is exactly those four:
//!
//! 1. it is "the sole issuer and revoker of SessionIds"; the adapter contributes only the link kind
//!    and the connection generation that scope them;
//! 2. "Within one connection generation, the coordinator never issues the same nonzero SessionId
//!    twice, including after its earlier session terminates" — the allocator here is monotonic for
//!    the device's whole life, which is strictly stronger and needs no used-set;
//! 3. a SessionId "is valid only with its link kind, principal scope, and connection generation",
//!    so every use is checked against the three facts that scoped it;
//! 4. §13's tombstone rule: a session released in this generation is remembered and its late frames
//!    are silently discarded, while an identifier that was never issued to this connection is
//!    untrusted framing.
//!
//! §13's tombstone set is kept as the **half-open interval** between the first SessionId this
//! connection was issued and the allocator's next value. §3 makes that allocator monotonic and
//! never-reusing, so the interval is exact, O(1), and — unlike a bounded per-identifier history —
//! cannot truncate. Truncation would turn ordinary in-flight traffic into a transport close, which
//! §13 forbids in as many words: the receiver "silently discards frames bearing one, sending
//! neither data acknowledgement nor fault and never closing the transport".
//!
//! The bounded [`ISSUE_HISTORY`] ring is kept for a narrower job: naming *which* of the three
//! scoping facts a control request got wrong. A classification that has aged out of it degrades to
//! `staleConnection`, which is a legal detail for a control refusal; the tombstone decision never
//! consults it.

use core::fmt;

use crate::error::detail;
use crate::hello::LinkKind;
use crate::ids::SessionId;

/// How many link kinds a device serves at once: BLE, USB, and the test link (§5).
const LINK_KINDS: usize = 3;

/// How many past issues are remembered with their exact owner, for detail classification only.
///
/// An identifier older than this is still rejected — it is below the allocator's cursor, so it was
/// certainly issued once — but it is reported as a stale connection rather than as a wrong link.
/// §13's tombstone decision does **not** use this ring: it uses the per-connection interval, which
/// cannot truncate.
pub const ISSUE_HISTORY: usize = 8;

/// The opaque stable principal-scope digest of §3.
///
/// It is an identity, not a cable: "Wherever the same authenticated identity is established on two
/// link kinds it is one principal scope". The engine never interprets the bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrincipalScope([u8; 16]);

impl PrincipalScope {
    /// Wraps a 16-byte digest the adapter established.
    pub const fn new(bytes: [u8; 16]) -> Self {
        PrincipalScope(bytes)
    }

    /// The digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Debug for PrincipalScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PrincipalScope(")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        write!(f, ")")
    }
}

/// The three facts a SessionId is scoped by, and the only thing an adapter contributes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkContext {
    /// Which physical link the record arrived on.
    pub link_kind: LinkKind,
    /// The authenticated identity behind it.
    pub principal: PrincipalScope,
    /// The connection generation. A reconnect increments it and makes every earlier session stale.
    pub generation: u32,
}

impl LinkContext {
    /// A context for `link_kind`, `principal`, and `generation`.
    pub const fn new(link_kind: LinkKind, principal: PrincipalScope, generation: u32) -> Self {
        LinkContext { link_kind, principal, generation }
    }

    /// True when both contexts name the same connection.
    pub fn is_same_connection(&self, other: &LinkContext) -> bool {
        self.link_kind == other.link_kind && self.principal == other.principal && self.generation == other.generation
    }
}

/// Why a named SessionId cannot be used, as one of §12's `invalidSession` details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRejection {
    /// This device never issued the identifier, or it is long released.
    Unknown,
    /// It belongs to an earlier connection generation of this link and principal.
    StaleConnection,
    /// It belongs to another authenticated identity.
    WrongPrincipal,
    /// It belongs to another link kind.
    WrongLink,
}

impl SessionRejection {
    /// The §12 `invalidSession` detail code.
    pub const fn detail(self) -> u16 {
        match self {
            SessionRejection::Unknown => detail::session::UNKNOWN,
            SessionRejection::StaleConnection => detail::session::STALE_CONNECTION,
            SessionRejection::WrongPrincipal => detail::session::WRONG_PRINCIPAL,
            SessionRejection::WrongLink => detail::session::WRONG_LINK,
        }
    }
}

/// What a stream frame bearing some SessionId means to the receiver (§13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamAdmission {
    /// The frame belongs to the live session of this connection.
    Owned,
    /// A session this connection released in this generation: discard it in silence.
    Tombstoned,
    /// An identifier never issued to this connection: untrusted framing, close the stream.
    Untrusted,
}

/// One remembered issue.
#[derive(Debug, Clone, Copy)]
struct Issue {
    session_id: SessionId,
    owner: LinkContext,
}

/// What one connection has been issued: its identity, and the allocator value it started from.
#[derive(Debug, Clone, Copy)]
struct Generation {
    context: LinkContext,
    first: u32,
}

/// The sole issuer and revoker of SessionIds.
#[derive(Debug)]
pub struct SessionCoordinator {
    next: u32,
    live: Option<Issue>,
    history: [Option<Issue>; ISSUE_HISTORY],
    cursor: usize,
    generations: [Option<Generation>; LINK_KINDS],
}

impl Default for SessionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCoordinator {
    /// A coordinator with nothing issued.
    pub const fn new() -> Self {
        SessionCoordinator {
            next: 1,
            live: None,
            history: [None; ISSUE_HISTORY],
            cursor: 0,
            generations: [None; LINK_KINDS],
        }
    }

    /// Records that a connection generation has opened on one link.
    ///
    /// The allocator's current value becomes that connection's first issuable SessionId, which is
    /// the lower bound of its §13 tombstone interval. A reconnect calls this again and the old
    /// interval goes with the old connection, "since a new generation makes every earlier SessionId
    /// stale by the generation check alone".
    pub fn open(&mut self, context: LinkContext) {
        self.generations[Self::index(context.link_kind)] = Some(Generation { context, first: self.next });
    }

    const fn index(link_kind: LinkKind) -> usize {
        match link_kind {
            LinkKind::Ble => 0,
            LinkKind::Usb => 1,
            LinkKind::Test => 2,
        }
    }

    /// True when `session_id` falls inside the half-open interval this connection was issued from.
    fn issued_to(&self, session_id: SessionId, context: &LinkContext) -> bool {
        match self.generations[Self::index(context.link_kind)] {
            Some(generation) if generation.context.is_same_connection(context) => {
                session_id.get() >= generation.first && session_id.get() < self.next
            }
            _ => false,
        }
    }

    /// The live session, if one is attached.
    pub fn live(&self) -> Option<SessionId> {
        self.live.map(|issue| issue.session_id)
    }

    /// The connection that owns the live session.
    pub fn live_owner(&self) -> Option<LinkContext> {
        self.live.map(|issue| issue.owner)
    }

    /// Issues a fresh SessionId to `owner`, atomically revoking whatever was live.
    ///
    /// §6.1: a same-intent in-progress operation "receives a fresh SessionId bound to the current
    /// connection and the same reservation/work; issuing it atomically revokes any SessionId
    /// previously bound to that work, so at most one session is ever live for one work record".
    /// Returns `None` only when the nonzero `u32` space is exhausted, which §3 makes the adapter's
    /// job to avoid by reconnecting first.
    pub fn issue(&mut self, owner: LinkContext) -> Option<SessionId> {
        let session_id = SessionId::new(self.next)?;
        self.next = self.next.checked_add(1)?;
        if let Some(previous) = self.live.take() {
            self.remember(previous);
        }
        self.live = Some(Issue { session_id, owner });
        Some(session_id)
    }

    /// Releases the live session, leaving a tombstone behind.
    pub fn revoke(&mut self) {
        if let Some(issue) = self.live.take() {
            self.remember(issue);
        }
    }

    /// Releases the live session only when `owner` is the connection that holds it.
    ///
    /// §3: "Wrong-owner stream, finish, checkpoint, or disconnect handling cannot advance or
    /// release a current session." Returns whether anything was released.
    pub fn revoke_owned_by(&mut self, owner: &LinkContext) -> bool {
        match self.live {
            Some(issue) if issue.owner.is_same_connection(owner) => {
                self.revoke();
                true
            }
            _ => false,
        }
    }

    /// Checks a SessionId a control request named, against the connection it arrived on.
    pub fn check(&self, session_id: SessionId, context: &LinkContext) -> Result<(), SessionRejection> {
        if let Some(issue) = self.live {
            if issue.session_id == session_id {
                return Self::classify(&issue.owner, context);
            }
        }
        if session_id.get() >= self.next {
            return Err(SessionRejection::Unknown);
        }
        match self.remembered(session_id) {
            // A released session of this very connection no longer exists to be named: §6.3 makes a
            // FinishUpload against one `invalidSession`, and the identifier itself is not stale.
            Some(issue) => Err(Self::classify(&issue.owner, context).err().unwrap_or(SessionRejection::Unknown)),
            None => Err(SessionRejection::StaleConnection),
        }
    }

    /// Classifies a stream frame's SessionId (§13).
    ///
    /// The tombstone test is the interval, not the history ring: every identifier this connection
    /// was issued and no longer owns is discarded in silence, however many have been issued since.
    pub fn admit_stream(&self, session_id: SessionId, context: &LinkContext) -> StreamAdmission {
        if let Some(issue) = self.live {
            if issue.session_id == session_id {
                return if issue.owner.is_same_connection(context) {
                    StreamAdmission::Owned
                } else {
                    // Live, but never issued to *this* connection: untrusted framing here.
                    StreamAdmission::Untrusted
                };
            }
        }
        if self.issued_to(session_id, context) {
            StreamAdmission::Tombstoned
        } else {
            StreamAdmission::Untrusted
        }
    }

    /// Drops every session belonging to a connection that has gone away.
    ///
    /// §13: link teardown "calls the transfer coordinator once with the exact `(link kind,
    /// principal scope, connection generation)`; stale teardown is a no-op".
    pub fn on_link_lost(&mut self, context: &LinkContext) -> bool {
        self.revoke_owned_by(context)
    }

    fn classify(owner: &LinkContext, context: &LinkContext) -> Result<(), SessionRejection> {
        if owner.link_kind != context.link_kind {
            return Err(SessionRejection::WrongLink);
        }
        if owner.principal != context.principal {
            return Err(SessionRejection::WrongPrincipal);
        }
        if owner.generation != context.generation {
            return Err(SessionRejection::StaleConnection);
        }
        Ok(())
    }

    fn remember(&mut self, issue: Issue) {
        self.history[self.cursor] = Some(issue);
        self.cursor = (self.cursor + 1) % ISSUE_HISTORY;
    }

    fn remembered(&self, session_id: SessionId) -> Option<Issue> {
        self.history.iter().flatten().find(|issue| issue.session_id == session_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(link_kind: LinkKind, principal: u8, generation: u32) -> LinkContext {
        LinkContext::new(link_kind, PrincipalScope::new([principal; 16]), generation)
    }

    #[test]
    fn an_identifier_is_never_issued_twice_in_one_generation() {
        let mut coordinator = SessionCoordinator::new();
        let owner = context(LinkKind::Ble, 1, 1);
        let first = coordinator.issue(owner).unwrap();
        coordinator.revoke();
        let second = coordinator.issue(owner).unwrap();
        assert_ne!(first, second);
        assert_eq!(coordinator.live(), Some(second));
    }

    #[test]
    fn issuing_revokes_the_session_bound_to_the_same_work() {
        let mut coordinator = SessionCoordinator::new();
        let owner = context(LinkKind::Ble, 1, 1);
        coordinator.open(owner);
        let first = coordinator.issue(owner).unwrap();
        let second = coordinator.issue(owner).unwrap();
        assert_eq!(coordinator.check(first, &owner), Err(SessionRejection::Unknown));
        assert_eq!(coordinator.check(second, &owner), Ok(()));
        assert_eq!(coordinator.admit_stream(first, &owner), StreamAdmission::Tombstoned);
    }

    #[test]
    fn each_of_the_three_scoping_facts_has_its_own_detail() {
        let mut coordinator = SessionCoordinator::new();
        let owner = context(LinkKind::Ble, 1, 1);
        let session = coordinator.issue(owner).unwrap();

        assert_eq!(coordinator.check(session, &context(LinkKind::Usb, 1, 1)), Err(SessionRejection::WrongLink));
        assert_eq!(coordinator.check(session, &context(LinkKind::Ble, 2, 1)), Err(SessionRejection::WrongPrincipal));
        assert_eq!(coordinator.check(session, &context(LinkKind::Ble, 1, 2)), Err(SessionRejection::StaleConnection));
        assert_eq!(coordinator.check(SessionId::new(9_999).unwrap(), &owner), Err(SessionRejection::Unknown));
    }

    #[test]
    fn a_wrong_owner_cannot_release_the_live_session() {
        let mut coordinator = SessionCoordinator::new();
        let owner = context(LinkKind::Ble, 1, 1);
        let session = coordinator.issue(owner).unwrap();
        assert!(!coordinator.revoke_owned_by(&context(LinkKind::Usb, 1, 1)));
        assert!(!coordinator.on_link_lost(&context(LinkKind::Ble, 1, 2)));
        assert_eq!(coordinator.live(), Some(session));
        assert!(coordinator.on_link_lost(&owner));
        assert_eq!(coordinator.live(), None);
    }

    #[test]
    fn a_live_identifier_from_another_connection_is_untrusted_framing() {
        let mut coordinator = SessionCoordinator::new();
        let owner = context(LinkKind::Ble, 1, 1);
        coordinator.open(owner);
        let session = coordinator.issue(owner).unwrap();
        assert_eq!(coordinator.admit_stream(session, &owner), StreamAdmission::Owned);
        assert_eq!(coordinator.admit_stream(session, &context(LinkKind::Usb, 1, 1)), StreamAdmission::Untrusted);
        assert_eq!(
            coordinator.admit_stream(SessionId::new(4_000).unwrap(), &owner),
            StreamAdmission::Untrusted,
            "an identifier this device never issued closes the stream rather than being discarded"
        );
    }

    #[test]
    fn a_tombstone_survives_any_number_of_later_issues() {
        // §13: the receiver "keeps a tombstone for every session it released in this generation and
        // silently discards frames bearing one ... never closing the transport". A bounded history
        // would turn the oldest of these into a transport close, which is what the interval avoids.
        let mut coordinator = SessionCoordinator::new();
        let owner = context(LinkKind::Ble, 1, 1);
        coordinator.open(owner);
        let first = coordinator.issue(owner).unwrap();
        for _ in 0..ISSUE_HISTORY * 4 {
            coordinator.issue(owner).unwrap();
        }
        assert_eq!(coordinator.admit_stream(first, &owner), StreamAdmission::Tombstoned);
        // The control plane still refuses it; only the detail degrades once the ring has aged out.
        assert_eq!(coordinator.check(first, &owner), Err(SessionRejection::StaleConnection));
    }

    #[test]
    fn an_identifier_from_an_earlier_generation_is_untrusted_rather_than_tombstoned() {
        let mut coordinator = SessionCoordinator::new();
        let first_generation = context(LinkKind::Ble, 1, 1);
        coordinator.open(first_generation);
        let session = coordinator.issue(first_generation).unwrap();
        assert_eq!(coordinator.admit_stream(session, &first_generation), StreamAdmission::Owned);

        let second_generation = context(LinkKind::Ble, 1, 2);
        coordinator.open(second_generation);
        assert_eq!(coordinator.admit_stream(session, &second_generation), StreamAdmission::Untrusted);
        assert_eq!(coordinator.check(session, &second_generation), Err(SessionRejection::StaleConnection));
    }
}
