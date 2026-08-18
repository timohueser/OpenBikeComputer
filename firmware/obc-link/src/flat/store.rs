//! The store seam, as the engine names it, and the two policy hooks that are nobody else's.
//!
//! `FLAT_Store_Protocol.md` §2 declares five operations plus `entries` and `journal`. This module is
//! that seam **restated in the crate that consumes it**, because the dependency runs downward: the
//! engine is a foundation crate and the flat store is a platform adapter, so `firmware/tools/
//! dependency_rules.json` forbids `obc-link -> obc-storage`. The store therefore implements what the
//! engine declares; the binder is one `impl` block and it is where the two definitions are pinned to
//! each other.
//!
//! What the engine does **not** name is as load-bearing as what it does. There is no block, no
//! extent, no LBA, no path and no filename here; [`Allocation`](Store::Allocation) is an opaque token
//! and [`Handle`](Store::Handle) is another. `journal` is absent because the ride is not a wire
//! transfer — it is FS8's, and the engine has no business checkpointing one.
//!
//! Two seam laws are worth restating where they are used, because breaking either leaks a row until
//! the card is remounted:
//!
//! - **`cancel` and `close` are mandatory on every abandonment path.** A dropped allocation or a
//!   dropped handle releases nothing.
//! - **`next_object_id` reserves nothing.** It is the id a create names, and it is safe only because
//!   §1 serves one transfer at a time.

use super::ids::{EntryMeta, ObjectId, ObjectKind, Revision, StoreId};

/// Why a mounted store refuses writes, or refuses everything (`FLAT_Store_Format.md` §5.6).
///
/// Mirrors the store's own classification variant for variant rather than collapsing it, so that
/// §3.9's `readOnly` details are chosen by the engine — where they are tested — and the binder is a
/// table with nothing to decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReadWrite,
    /// An object reached `Revision` `u64::MAX`. Reads are still served.
    RevisionSpaceExhausted,
    /// The commit sequence has no successor. Reads are still served.
    SequenceSpaceExhausted,
    /// No catalog copy validated. Nothing is readable.
    CatalogUnreadable,
    /// The card is not a flat store.
    Unformatted,
    /// The card is smaller than its superblock recorded: not the card that was formatted.
    CardTooSmall,
}

impl Mode {
    /// True when a commit may run.
    pub fn writable(self) -> bool {
        self == Mode::ReadWrite
    }

    /// True when the catalog is usable. Only the two exhausted cases still serve reads.
    pub fn readable(self) -> bool {
        matches!(self, Mode::ReadWrite | Mode::RevisionSpaceExhausted | Mode::SequenceSpaceExhausted)
    }
}

/// What an operation at the seam fails with. `FLAT_Store_Protocol.md` §2, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    RevisionConflict {
        current: Revision,
    },
    NoSpace {
        required: u64,
    },
    TooFragmented,
    CatalogFull,
    /// The seam's catch-all refusal. It is **not** always the client's fault: a full reservation or
    /// hold table lands here too, and §3.9's answer to that is `busy`, never `invalidRequest`.
    Invalid,
    Media,
    ReadOnly,
}

/// Where a `Put`'s extents come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutSource<A> {
    /// Publish the extents of a freshly written allocation, consuming it.
    Fresh(A),
    /// Keep the extents the named entry already holds and change only its metadata.
    Amend,
}

/// One entry mutation. A commit applies a batch of them atomically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation<A> {
    /// Publish a revision.
    Put { meta: EntryMeta, source: PutSource<A> },
    /// Remove one entry. Its extents are free at the gate.
    Remove { id: ObjectId, revision: Revision },
}

/// The card, as the engine sees it.
///
/// **Every method takes `&self`, the mutators included**, mirroring the store's own seam after
/// #1256's owner ruling of 2026-08-18: a store is shared, not owned. A board holds a source per
/// mounted shard for the life of the image, and a `&mut` write half made an engine that could commit
/// while a map was mounted un-expressible. The store carries the interior mutability; the engine is
/// simply a caller that no longer demands exclusivity, which is why [`Engine`](super::engine::Engine)
/// takes `store: &S` everywhere it used to take `&mut S`.
pub trait Store {
    /// An opaque reservation of extents, released by [`commit`](Store::commit) or
    /// [`cancel`](Store::cancel) and by nothing else.
    type Allocation: Copy;
    /// An open object. Keeps reading the revision it resolved until it is
    /// [`close`](Store::close)d.
    type Handle;

    /// Why this store refuses writes, if it does.
    fn mode(&self) -> Mode;

    /// The card's identity, which every `LIST` page carries.
    fn store_id(&self) -> StoreId;

    /// The catalog commit sequence: the staleness hint a paged listing is checked against.
    fn commit_sequence(&self) -> u64;

    /// The next `ObjectId` the cursor will hand out. **Reading it reserves nothing** — a create
    /// names it, and the commit that publishes the create is what advances the cursor.
    fn next_object_id(&self) -> ObjectId;

    /// Reserve space for `bytes`. RAM state until a commit names it.
    fn allocate(&self, bytes: u64) -> Result<Self::Allocation, StoreError>;

    /// Append to an allocation. A `write` that returns `Err` has advanced it by nothing: the same
    /// bytes may be written again, or the transfer abandoned through [`cancel`](Store::cancel).
    fn write(&self, allocation: &mut Self::Allocation, bytes: &[u8]) -> Result<(), StoreError>;

    /// Release a reservation without publishing it. Mandatory on every abandonment path.
    fn cancel(&self, allocation: Self::Allocation);

    /// Apply `mutations` atomically and return the new catalog commit sequence. A `commit` that
    /// returns `Err` changed nothing.
    fn commit(&self, mutations: &[Mutation<Self::Allocation>]) -> Result<u64, StoreError>;

    /// Resolve an object. `None` takes the head; `Some(r)` takes exactly that revision.
    fn open(&self, id: ObjectId, revision: Option<Revision>) -> Result<Self::Handle, StoreError>;

    /// Random access inside an open object. Returns bytes read, short only at end of payload.
    fn read(&self, handle: &Self::Handle, offset: u64, buf: &mut [u8]) -> Result<usize, StoreError>;

    /// Close an open object. Mandatory: a dropped handle leaks its row and its extents.
    fn close(&self, handle: Self::Handle);

    /// The read-only catalog view, in the catalog's own `(ObjectId, Revision)` order.
    fn entries(&self) -> impl Iterator<Item = EntryMeta> + '_;

    /// True when the last [`entries`](Store::entries) listing ran to the end of the array. A short
    /// listing is a media failure with nowhere to report itself, so **every** caller that treats a
    /// listing as the catalog asks here before it does.
    fn entries_ok(&self) -> bool;
}

/// The two decisions the engine cannot make for itself, and the crate they belong to cannot reach.
///
/// A kind's validator parses OBCR, OBCW, OBCM or OBCU, and arming an update needs `obc-dfu`, the
/// RRAM boot page and a reboot. Both sit *above* this crate in the dependency graph, so both arrive
/// as a hook the board fills in. Every method has a default, so a board that has neither implements
/// this with an empty block and gets §3.6's "no validator" and §4's "this device cannot arm".
///
/// The detail values a refusal carries are the kind's own: §3.9 gives `rejected` a detail space and
/// says the kind's validator owns it.
pub trait Policy {
    /// §3.6's "runs the kind's validator": the upload is complete and its whole-payload CRC has
    /// checked out, and this is the last word before the commit. A refusal costs nothing but the
    /// allocation the engine then cancels.
    ///
    /// It is deliberately whole-payload rather than streaming. The kinds that need validating —
    /// OBCR, OBCW, OBCM, OBCU — are all read from their own header outward, and the engine has no
    /// buffer to offer a validator that wanted the bytes twice.
    fn accept(&mut self, kind: ObjectKind, payload_len: u64) -> Result<(), u16> {
        let _ = (kind, payload_len);
        Ok(())
    }

    /// §4 step 1: validate the pinned package — OBCU structure, image CRC, signature, version
    /// monotonicity, ride and battery state — and report how many bytes the rollback reserve needs.
    ///
    /// The default refuses, because a device with no update path must not commit a reserve it can
    /// never hand off.
    fn validate_package(&mut self, package: ObjectId, revision: Revision) -> Result<u64, u16> {
        let _ = (package, revision);
        Err(0)
    }

    /// §4 step 3: write both extent lists into the RRAM boot page and read it back. The engine has
    /// committed the reserve by the time this is called and names both entries; resolving them to
    /// block runs is below the seam and therefore the binder's, not the engine's.
    fn hand_off(&mut self, package: (ObjectId, Revision), reserve: (ObjectId, Revision)) -> Result<(), u16> {
        let _ = (package, reserve);
        Err(0)
    }
}

/// A device with no kind validators and no update path: every payload is accepted and `ARM` is
/// refused. It is what the host harness runs and what a board wires until FS7 and FS9 land.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenPolicy;

impl Policy for OpenPolicy {}
