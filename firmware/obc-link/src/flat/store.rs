//! The store seam, as the engine names it, and the two policy hooks that are nobody else's.
//!
//! The seam is restated in the crate that consumes it, because the dependency runs downward:
//! `obc-link` is a foundation crate and the flat store is a platform adapter, so the store
//! implements what the engine declares and the binder pins the two definitions to each other.
//! There is no block, no extent, no LBA and no filename here: an allocation and a handle are opaque
//! tokens.
//!
//! Two seam laws, because breaking either leaks a row until the card is remounted: `cancel` and
//! `close` are mandatory on every abandonment path, and `next_object_id` reserves nothing, which is
//! safe only because the device serves one transfer at a time.

use super::ids::{EntryMeta, ObjectId, ObjectKind, Revision, StoreId};

/// Why a mounted store refuses writes, or refuses everything.
///
/// It mirrors the store's own classification variant for variant, so the engine chooses the
/// `readOnly` details and the binder is a table with nothing to decide.
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
    pub fn writable(self) -> bool {
        self == Mode::ReadWrite
    }

    /// True when the catalog is usable. Only the two exhausted cases still serve reads.
    pub fn readable(self) -> bool {
        matches!(self, Mode::ReadWrite | Mode::RevisionSpaceExhausted | Mode::SequenceSpaceExhausted)
    }
}

/// What an operation at the seam fails with.
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
    /// The seam's catch-all refusal. It is not always the client's fault: a full reservation table
    /// lands here too, and the answer to that is `busy`, never `invalidRequest`.
    Invalid,
    Media,
    ReadOnly,
    /// Try again: every hold row is taken, so an `open` has no slot to resolve into.
    ///
    /// It is `busy` with the `holds` detail on the wire, and distinct from
    /// [`Invalid`](StoreError::Invalid) because the two answer opposite questions for a client's
    /// retry policy: this request is wrong, against someone else is reading right now.
    Busy,
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
    Put {
        meta: EntryMeta,
        source: PutSource<A>,
    },
    /// Remove one entry. Its extents are free at the gate.
    Remove {
        id: ObjectId,
        revision: Revision,
    },
}

/// Exact finalized ride bytes held in a durable client archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveSource {
    pub store: StoreId,
    pub id: ObjectId,
    pub revision: Revision,
    pub payload_len: u64,
    pub payload_crc: u32,
}

/// The current card sequence and original retention timestamp of a validated archive proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveResult {
    pub sequence: u64,
    pub timestamp: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveError {
    Unsupported,
    SourceMismatch,
    Store(StoreError),
}

/// The card, as the engine sees it.
///
/// Every method takes `&self`, the mutators included: a store is shared, not owned. A board holds a
/// source per mounted shard for the life of the image, so the store carries the interior mutability
/// and the engine is a caller that demands no exclusivity.
pub trait Store {
    /// An opaque reservation of extents, released by [`commit`](Store::commit) or
    /// [`cancel`](Store::cancel) and by nothing else.
    type Allocation: Copy;
    /// An open object. Keeps reading the revision it resolved until it is
    /// [`close`](Store::close)d.
    type Handle;

    fn mode(&self) -> Mode;

    /// The card's identity, which every `LIST` page carries.
    fn store_id(&self) -> StoreId;

    /// The catalog commit sequence: the staleness hint a paged listing is checked against.
    fn commit_sequence(&self) -> u64;

    /// The next `ObjectId` the cursor will hand out. Reading it reserves nothing: the commit that
    /// publishes a create is what advances the cursor.
    fn next_object_id(&self) -> ObjectId;

    /// Reserve space for `bytes`. RAM state until a commit names it.
    fn allocate(&self, bytes: u64) -> Result<Self::Allocation, StoreError>;

    /// Append to an allocation. A `write` that returns `Err` has advanced it by nothing: the same
    /// bytes may be written again, or the transfer abandoned through [`cancel`](Store::cancel).
    fn write(&self, allocation: &mut Self::Allocation, bytes: &[u8]) -> Result<(), StoreError>;

    /// Release a reservation without publishing it. Mandatory on every abandonment path.
    fn cancel(&self, allocation: Self::Allocation);

    /// Apply `mutations` atomically and return the new catalog commit sequence. A `commit` that
    /// fails before publication leaves the old catalog authoritative. An uncertain gate error
    /// fences mutations until remount; callers must not assume that every error means no commit.
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
    /// listing is a media failure with nowhere else to report itself, so every caller that treats a
    /// listing as the catalog asks here first.
    fn entries_ok(&self) -> bool;

    /// Persist exact archive possession without starting a retention countdown. An existing exact
    /// proof is idempotent. Success requires validated durable metadata, never GET completion.
    fn archive_ride(&self, source: ArchiveSource) -> Result<ArchiveResult, ArchiveError> {
        let _ = source;
        Err(ArchiveError::Unsupported)
    }

    /// Destructively initialize the underlying media as an empty flat store. The in-memory store is
    /// intentionally not updated: a successful call is followed by a link drain and immediate
    /// reboot, so no caller may observe two store identities in one boot.
    fn format(&self, replacement: StoreId) -> Result<(), StoreError> {
        let _ = replacement;
        Err(StoreError::ReadOnly)
    }
}

pub trait Policy {
    fn accept(&mut self, kind: ObjectKind, payload_len: u64) -> Result<(), u16> {
        let _ = (kind, payload_len);
        Ok(())
    }

    /// Validate the pinned package and report how many bytes the rollback reserve needs.
    ///
    /// The default refuses, because a device with no update path must not commit a reserve it can
    /// never hand off.
    fn validate_package(&mut self, package: ObjectId, revision: Revision) -> Result<u64, u16> {
        let _ = (package, revision);
        Err(0)
    }

    /// Write both extent lists into the RRAM boot page and read it back. The reserve is committed
    /// by the time this is called; resolving the entries to block runs is below the seam.
    fn hand_off(&mut self, package: (ObjectId, Revision), reserve: (ObjectId, Revision)) -> Result<(), u16> {
        let _ = (package, reserve);
        Err(0)
    }
}

/// A device with no kind validators and no update path: every payload is accepted and `ARM` is
/// refused.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenPolicy;

impl Policy for OpenPolicy {}
