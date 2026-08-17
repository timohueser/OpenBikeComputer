//! The typed repositories: §2's sixth owner, one concrete type per object kind.
//!
//! §2 of the system contract gives them a job list no other owner has — "authorization, semantic
//! validation, catalog projections, metadata policy, and domain commands" — and then two rules about
//! their shape that this module is built around:
//!
//! > CardStore does not grow a union of every domain method: it lends transaction, lease, and
//! > catalog capabilities to one concrete repository at a time.
//!
//! > Storage-private `GenerationId` and paths do not cross the public repository/client seam.
//!
//! ## Two halves, and why there have to be two
//!
//! A repository is a *resident* half and a *borrowed* half, and the split is forced rather than
//! stylistic.
//!
//! The resident half — [`route::RouteRepository`] and its siblings — is the rule set: what a
//! payload of this kind has to look like, what the catalog projection of one is, what a metadata
//! patch means. It lives inside the store because the commit path calls it, at the one moment §7
//! allows: after the seal and before the publication. That call arrives through the kernel's
//! [`Validator`](super::transaction::Validator) seam.
//!
//! The borrowed half — [`route::Routes`] and its siblings — is the semantic API a caller drives:
//! resolve, list, plan a Put, read the metadata policy. It cannot be resident, because it needs the
//! store, and the store owns it. So it is handed out for the duration of one call chain and given
//! back, which is exactly "lends … to one concrete repository at a time" — enforced by the borrow
//! checker rather than by a comment, since [`CardStore::routes`](super::store::CardStore::routes)
//! takes `&mut self` and the view holds that borrow.
//!
//! ## What is deliberately not here
//!
//! There is **no repository trait**. [`Routes`](route::Routes), [`Trips`](trip::Trips) and
//! [`Weather`](weather::Weather) share [`Capability`] — the lent capability — and nothing else: no
//! supertrait, no associated-type dance, no `dyn Repository`. Every one of them has methods the
//! others must not have (a route has retention and a display name; weather has a singleton identity
//! and a durable request context; a trip has ordered stages), and a trait that was the union of
//! those would put every domain's vocabulary in front of every domain's caller — which is the exact
//! shape #1256 forbids.
//!
//! [`route`] is complete for its slice. [`trip`] and [`weather`] are deliberately shaped rather than
//! finished: their payload formats belong to DOS5 and DOS9, and what they carry now is the seam,
//! the per-kind policy the registries already fix, and an explicit statement of what they do not
//! yet derive.

pub mod route;
pub mod trip;
pub mod weather;

use core::marker::PhantomData;

use obc_link::engine::FailureCause;
use obc_link::error::detail;
use obc_link::ids::{LogicalObjectId, Revision, StoreId};
use obc_link::registry::ObjectKind;

use super::commit::CommitLog;
use super::entries::{CatalogHead, HeadKey};
use super::transaction::{CatalogProjection, KernelMedia, KernelTransaction, SealedBytes, Validation, Validator};

pub use route::{RouteRepository, Routes};
pub use trip::{TripRepository, Trips};
pub use weather::{Weather, WeatherRepository};

/// One published head, in the vocabulary a repository is allowed to speak.
///
/// §2: "Storage-private `GenerationId` and paths do not cross the public repository/client seam."
/// So there is no generation here, no filename, no journal slot and no shard — a caller gets the
/// logical identity, the concurrency token, and the two facts a transfer needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadView {
    /// The logical object.
    pub logical_object_id: LogicalObjectId,
    /// The revision it stands at, which is its compare-and-swap token.
    pub revision: Revision,
    /// The payload length.
    pub length: u64,
    /// The payload CRC-32.
    pub crc32: u32,
}

/// The capabilities `CardStore` lends to exactly one repository at a time.
///
/// §2 names them: "transaction, lease, and catalog capabilities". This is that lending made
/// concrete — a repository reaches the store through this and through nothing else, so the set of
/// things a repository *can* do is a list one can read rather than the whole of `KernelTransaction`.
///
/// It carries two of the three properties [`CardStore`](super::store::CardStore)'s lock law rests
/// on, and it is worth being exact about which:
///
/// - **It cannot escape or alias.** It is a `&mut` borrow of the store, so a view cannot outlive the
///   call that was lent it and no second view can exist beside it. That is the borrow checker, and
///   it is airtight.
/// - **It cannot travel.** The [`PhantomData`] below is a raw pointer, which is neither `Send` nor
///   `Sync`, so a view — and any future that holds one across a suspension — is `!Send`. A view
///   cannot be moved to another thread or executor, and a `spawn` with a `Send` bound refuses the
///   future outright.
/// - **It is not stopped from being held across an `.await`.** Nothing in the type system prevents
///   that on a single-threaded executor, which is what the board runs. That third property is a
///   discipline the board glue keeps, stated as one in
///   [`CardStore`](super::store::CardStore)'s module documentation rather than claimed as a
///   compiler guarantee.
pub struct Capability<'a, M: KernelMedia> {
    transaction: &'a mut KernelTransaction<M, DomainRepositories, StoreHooks>,
    /// Removes `Send` and `Sync`. A raw pointer is the standard marker for it, and `*const ()`
    /// carries no variance or drop implications of its own.
    not_send: PhantomData<*const ()>,
}

impl<'a, M: KernelMedia> Capability<'a, M> {
    /// Lends the store's capabilities. Crate-private: only `CardStore` may hand these out.
    pub(super) fn new(transaction: &'a mut KernelTransaction<M, DomainRepositories, StoreHooks>) -> Self {
        Capability { transaction, not_send: PhantomData }
    }

    /// The store this repository is a view of.
    pub fn store_id(&self) -> StoreId {
        self.transaction.store_id()
    }

    /// §4's repository revision: the durable, monotonic token this kind's mutations advance.
    ///
    /// It is read from the projection's own per-kind row rather than derived from the heads, because
    /// §4 makes it monotonic and a maximum over live heads would fall back when the last one was
    /// deleted.
    pub fn revision(&self, kind: ObjectKind) -> Revision {
        self.transaction
            .index()
            .repositories
            .iter()
            .find(|row| row.kind == kind.to_u16())
            .map_or(Revision::ZERO, |row| row.revision)
    }

    /// The head one logical object stands at, from the resident index and with no card read.
    pub fn head(&self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Option<HeadView> {
        self.transaction.index().head(HeadKey { kind: kind.to_u16(), id: logical_object_id }).map(|entry| HeadView {
            logical_object_id: entry.id,
            revision: entry.revision,
            length: entry.length,
            crc32: entry.crc,
        })
    }

    /// One page of this kind's heads in logical-ID order, starting after `after`.
    ///
    /// The index keeps heads sorted by `(kind, logical id)`, so a page is a contiguous run and the
    /// cursor is the last ID of the previous page — no snapshot, no offset arithmetic, and the same
    /// ordering §4 gives snapshot catalog paging.
    pub fn page(&self, kind: ObjectKind, after: Option<LogicalObjectId>, out: &mut [HeadView]) -> usize {
        let mut filled = 0;
        for entry in self.transaction.index().heads.iter() {
            if filled == out.len() {
                break;
            }
            if entry.kind != kind.to_u16() {
                continue;
            }
            if after.is_some_and(|after| entry.id.get() <= after.get()) {
                continue;
            }
            out[filled] = HeadView {
                logical_object_id: entry.id,
                revision: entry.revision,
                length: entry.length,
                crc32: entry.crc,
            };
            filled += 1;
        }
        filled
    }

    /// §3's reserved weather singleton, which store initialization allocates and every mount keeps.
    ///
    /// Two sources, in the order they become true. Once a durable weather request context exists it
    /// names the identity outright. Before that — which is every store that has never been asked for
    /// weather, including a freshly initialized one — the reservation is the weather repository's own
    /// logical-ID cursor, which §12's initial projection writes precisely so the identity exists
    /// "even when no weather head exists". A store with no weather repository row at all has none.
    pub fn weather_singleton(&self) -> Option<LogicalObjectId> {
        if let Some(state) = self.transaction.index().weather {
            return Some(state.logical_id);
        }
        self.transaction
            .index()
            .repositories
            .iter()
            .find(|row| row.kind == ObjectKind::Weather.to_u16())
            .map(|row| row.next_logical_id)
    }

    /// How many heads of this kind the store holds.
    pub fn count(&self, kind: ObjectKind) -> usize {
        self.transaction.index().heads.iter().filter(|entry| entry.kind == kind.to_u16()).count()
    }

    /// The catalog projection a head carries, copied into `into`, re-read from §13's newest source.
    ///
    /// This is a **card read**: §13 leaves the envelope on the card and re-reads it on demand, so a
    /// caller that wants every route's metadata pays one bounded read per route and should say so.
    pub fn projection(
        &mut self,
        kind: ObjectKind,
        logical_object_id: LogicalObjectId,
        into: &mut [u8],
    ) -> Result<Option<usize>, FailureCause> {
        self.transaction.head_projection(HeadKey { kind: kind.to_u16(), id: logical_object_id }, into)
    }

    /// The free bytes admission reserves against.
    pub fn free_bytes(&mut self) -> u64 {
        self.transaction.media_mut().free_bytes()
    }

    /// The store's commit log, for a repository that wants to see its own edges.
    pub fn commits(&mut self) -> &mut CommitLog {
        &mut self.transaction.hooks_mut().commits
    }
}

/// The per-kind repositories, resident in the store, reached through the kernel's validator seam.
///
/// It **dispatches**; it does not unify. Each arm below calls a different concrete type with its own
/// rules, its own detail namespace and its own idea of what a catalog projection is; nothing here
/// requires two domains to agree on a method signature, which is the whole point of §2's "concrete
/// borrowed repositories". A kind with no repository yet publishes §5.3's bare reservation rather
/// than a projection somebody invented for it.
#[derive(Debug, Default, Clone, Copy)]
pub struct DomainRepositories {
    /// The route rules (§4.1/§4.2/§4.3 route rows, and the OBCR payload).
    pub routes: RouteRepository,
    /// The trip rules.
    pub trips: TripRepository,
    /// The weather rules.
    pub weather: WeatherRepository,
}

impl Validator for DomainRepositories {
    fn validate(&mut self, subject: &Validation<'_>, bytes: &mut dyn SealedBytes) -> Result<CatalogProjection, u16> {
        match subject.kind {
            ObjectKind::Route => self.routes.validate(subject, bytes),
            ObjectKind::Trip => self.trips.validate(subject, bytes),
            ObjectKind::Weather => self.weather.validate(subject, bytes),
            // Rides, volume manifests and update packages are DOS6's, DOS7's and DOS8's. Publishing
            // §5.3's reservation for them is the honest answer: the store has no rules for these
            // kinds, so it claims no knowledge of their metadata.
            ObjectKind::Ride | ObjectKind::VolumeManifest | ObjectKind::UpdatePackage => {
                Ok(CatalogProjection::RESERVATION)
            }
        }
    }
}

/// The policy points the store itself owns, and the one that matters: §4's commit events.
#[derive(Debug, Default, Clone)]
pub struct StoreHooks {
    /// The retained revision and coalescing wake every durable commit lands in.
    pub commits: CommitLog,
}

impl super::transaction::Hooks for StoreHooks {
    fn committed(&mut self, event: super::commit::CommitEvent) {
        self.commits.record(event);
    }
}

/// The catalog envelope a head carries, staged for a repository that is about to rewrite it.
///
/// It is [`CatalogHead::ENVELOPE_CAPACITY`] because that is what §5.3 reserves; a repository builds
/// its projection in one of these and hands the bytes to [`CatalogProjection::of`].
pub type EnvelopeBuffer = [u8; CatalogHead::ENVELOPE_CAPACITY];

/// An empty staging buffer.
pub const EMPTY_ENVELOPE: EnvelopeBuffer = [0; CatalogHead::ENVELOPE_CAPACITY];

/// The §12 refusal a repository preflight reports when a compare-and-swap cannot succeed.
pub(crate) fn revision_conflict(current: Revision) -> FailureCause {
    FailureCause::RevisionConflict { detail: detail::revision::OBJECT, current }
}

/// The §12 refusal for a target that is not there.
pub(crate) fn not_found() -> FailureCause {
    FailureCause::ObjectNotFound { detail: detail::not_found::LOGICAL_OBJECT }
}
