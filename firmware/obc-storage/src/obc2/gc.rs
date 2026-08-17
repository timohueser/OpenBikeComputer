//! Reachability and incremental garbage collection (`OBC2_Storage_Format.md` §9).
//!
//! §9 fixes what a generation may be reachable from: "catalog heads, each published manifest's
//! resolution generation and the children it names, the open draft parent and its sealed parts,
//! active operations and WORK records, ActiveRideState and its matching RIDE slot, retained previous
//! entries, the current update handoff, and live leases." [`classify`] is that list, evaluated
//! against the resident index; everything it does not reach is an orphan.
//!
//! The collector on top of it is deliberately small. §9: "GC processes at most one generation per
//! invocation, recomputes reachability under the CardStore lock immediately before deletion, and
//! stops on an unknown record or path." So a [`Collector::step`] examines exactly one leaf, decides
//! it afresh, and deletes at most that one — there is no batch, no queue, and no reachability
//! snapshot that could go stale between the decision and the unlink.
//!
//! ## Three rules that are easy to get subtly wrong
//!
//! **Conservative on unreadable evidence.** §9: "If a resolution generation cannot be read or its
//! count and length disagree, every generation that manifest could name is treated as reachable and
//! GC advances no further on that head; torn evidence never orphans children." So an unreadable
//! table does not make a manifest's children collectable — it makes *every* candidate uncollectable
//! until the evidence is readable again, because the collector cannot know which generations that
//! manifest would have named. [`Class::Unresolved`] is that answer, and it is never a deletion.
//!
//! **A lease is a reason even though it is RAM.** §9 makes leases RAM ownership facts, and the
//! durable side of one appears only when the leased head is displaced. A generation a reader is
//! streaming is therefore reachable from the lease table and from nothing on the card, which is
//! precisely why the table is consulted here and why recovery must clear the durable lease bit
//! *before* the first pass runs.
//!
//! **An absent shard is an empty shard.** Shard directories are created on first use (the DOS2 owner
//! decision of 2026-08-16), so most of the 512 of them do not exist on a young card. Enumerating one
//! that is absent yields no names and is not an error; [`ShardDirectory::next_leaf`] states that
//! obligation, and a collector that treated a missing directory as a fault would stop at shard zero
//! of every fresh store.
//!
//! ## What a step costs, stated rather than assumed
//!
//! §9 costs "a full reachability pass" at "at most eight bounded reads of 776 bytes", and that
//! figure is about **resolution generations** — at most eight, because that is the volume-manifest
//! head limit. It is not the whole cost of a step. The resolution-present bit is card-resident
//! (§6.3 makes it travel with the field it describes), so nothing in RAM says which heads have a
//! resolution at all, and [`classify`] asks [`ReachabilitySource::head_fields`] for every head it
//! has not already ruled the generation out against — up to 256 bounded reads, each served from the
//! active checkpoint or from one journal record through the head's own slot reference.
//!
//! That is a real per-step cost and §13 already owns measuring it, as "the per-step cost of an
//! incremental GC shard visit". It is bounded and it is paid once per collected file rather than
//! per byte, which is why the resident checks run first: a generation any head, retained entry,
//! lease, draft row, ride or handoff names is answered without touching the card at all.

use obc_link::ids::{GenerationId, LogicalObjectId};

use super::compaction::CardHeadFields;
use super::entries::ActiveOperation;
use super::index::{HeadIndexEntry, RamIndex};
use super::leases::LeaseTable;
use super::names::{LeafName, Role, SHARD_COUNT};
use super::resolution::{self, Resolution};

/// What a generation is reachable from (§9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    /// It is a published catalog head's payload.
    Head {
        /// The head's object kind.
        kind: u16,
        /// Its logical object ID.
        id: LogicalObjectId,
    },
    /// It is a published manifest head's resolution generation (§8).
    ResolutionTable {
        /// The manifest head's own generation.
        manifest: GenerationId,
    },
    /// A published manifest's resolution table names it as a child.
    ManifestChild {
        /// The manifest head's own generation.
        manifest: GenerationId,
    },
    /// It is the open draft parent's manifest, or the resolution generation it reserved.
    DraftParent,
    /// It is a draft part of the open parent.
    DraftPart,
    /// It is the reserved generation of an active operation.
    ActiveOperation,
    /// It is the prospective generation of the recording or recoverable ride.
    ActiveRide,
    /// A retained-previous entry names it (§9's three reasons).
    Retained,
    /// A live download lease pins it.
    Lease,
    /// The current update handoff names it as the package or its rollback snapshot.
    UpdateHandoff,
}

/// §9's classification of one generation file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Something still names it. Never collected.
    Referenced(Reference),
    /// An active claim reserved it but nothing has published it.
    ///
    /// Under the restart-only profile this is §7's "recovery classifies a claimed, unsealed
    /// generation as restartable work at offset zero" — the bytes are discarded on readmission, but
    /// the *file* belongs to a live claim and is not an orphan.
    ResumableWork,
    /// A manifest's resolution table could not be read, so no generation may be ruled out.
    ///
    /// §9: "torn evidence never orphans children." This is not a fault and not a deletion; the head
    /// whose table failed is reported degraded through §12's per-entry rule.
    Unresolved {
        /// The manifest head whose resolution generation would not read.
        manifest: GenerationId,
    },
    /// No record names it. Collectable.
    Orphan,
}

impl Class {
    /// Whether this class permits deletion. Only [`Orphan`](Class::Orphan) does.
    pub fn is_collectable(&self) -> bool {
        matches!(self, Class::Orphan)
    }
}

/// The bounded card reads reachability needs beyond the resident index (§6.3, §9).
pub trait ReachabilitySource {
    /// What a bounded read can fail with.
    type Error;

    /// The card-resident fields of one head: its catalog-projection envelope, and the resolution
    /// generation with the flag that travels with it.
    ///
    /// Sourced exactly as §6.3's compaction pass sources them — from the journal record the head's
    /// slot reference names, or from the active checkpoint's stored bytes.
    fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, Self::Error>;

    /// Reads a resolution generation into `into`, returning its length.
    ///
    /// `Ok(None)` means the generation could not be read at all, which §9 treats exactly as a table
    /// whose count and length disagree: conservative, never an orphan. An `Err` is a medium failure
    /// and stops the pass rather than deciding anything.
    fn resolution(
        &mut self,
        generation: GenerationId,
        into: &mut [u8; resolution::MAX_BODY_LEN],
    ) -> Result<Option<usize>, Self::Error>;
}

/// Classifies one generation against everything §9 lists.
///
/// `scratch` is the one bounded decode buffer a reachability pass needs — §9 costs a full pass at
/// "at most eight bounded reads of 776 bytes", one per volume-manifest head, into this buffer.
///
/// ## Why there is no separate WORK-record root
///
/// §9 lists "active operations **and WORK records**" as reachability sources, and only the first is
/// consulted here. They are the same root. §7: "`BeginWork` reserves the next GenerationId and the
/// preflighted logical resources in the catalog journal before either physical file is created" —
/// so a WORK record's generation is always one an active row already carries under
/// [`ActiveOperation::FLAG_GENERATION_RESERVED`], or one a draft row or `ActiveRideState` names, all
/// of which are checked. A `WORK` file whose generation none of them names is therefore an orphan by
/// construction rather than by omission, which is exactly what the collector's pair deletion assumes
/// when it removes a `WORK` leaf it never classified on its own.
pub fn classify<S: ReachabilitySource>(
    index: &RamIndex,
    leases: &LeaseTable,
    generation: GenerationId,
    source: &mut S,
    scratch: &mut [u8; resolution::MAX_BODY_LEN],
) -> Result<Class, S::Error> {
    // Everything resident first, cheapest and most certain, before a byte is read from the card.
    if let Some(head) = index.heads.iter().find(|head| head.generation == generation) {
        return Ok(Class::Referenced(Reference::Head { kind: head.kind, id: head.id }));
    }
    if index.retained.iter().any(|entry| entry.generation == generation) {
        return Ok(Class::Referenced(Reference::Retained));
    }
    if leases.holds(generation) {
        return Ok(Class::Referenced(Reference::Lease));
    }
    if let Some(parent) = &index.draft_parent {
        if parent.manifest_generation == generation || parent.resolution == generation {
            return Ok(Class::Referenced(Reference::DraftParent));
        }
    }
    if index.draft_parts.iter().any(|part| part.generation == generation) {
        return Ok(Class::Referenced(Reference::DraftPart));
    }
    if let Some(ride) = &index.ride {
        if ride.generation == generation {
            return Ok(Class::Referenced(Reference::ActiveRide));
        }
    }
    if let Some(handoff) = &index.handoff {
        if handoff.package_generation == generation
            || (handoff.flags & super::handoff::HandoffRef::FLAG_ROLLBACK_SNAPSHOT != 0
                && handoff.rollback_generation == generation)
        {
            return Ok(Class::Referenced(Reference::UpdateHandoff));
        }
    }

    // §9 walks the resolution table, never the manifest payload. One bounded read per manifest head,
    // and an unreadable one rules nothing out.
    for head in index.heads.iter() {
        let fields = source.head_fields(head)?;
        if !fields.resolution_present {
            continue;
        }
        if fields.resolution == generation {
            return Ok(Class::Referenced(Reference::ResolutionTable { manifest: head.generation }));
        }
        let Some(len) = source.resolution(fields.resolution, scratch)? else {
            return Ok(Class::Unresolved { manifest: head.generation });
        };
        let Ok(table) = Resolution::decode(&scratch[..len]) else {
            return Ok(Class::Unresolved { manifest: head.generation });
        };
        if table.iter().any(|entry| entry.generation == generation) {
            return Ok(Class::Referenced(Reference::ManifestChild { manifest: head.generation }));
        }
    }

    // Last, the claims: a generation an active row reserved is live work rather than an orphan.
    if index
        .actives
        .iter()
        .any(|row| row.flags & ActiveOperation::FLAG_GENERATION_RESERVED != 0 && row.generation == generation)
    {
        return Ok(Class::ResumableWork);
    }

    Ok(Class::Orphan)
}

/// Where an enumeration pass has reached (§9: "Its cursor is `(shard index, last name)`").
///
/// §9 holds this "for the lifetime of the mount and restarted from shard zero when the mount
/// restarts", which is why it is a RAM value with no encoder: nothing on the card records how far a
/// pass had got, and nothing needs to. A mount that restarts simply walks the tree again, and §9's
/// snapshot argument makes that safe — "a file created after its shard was visited is simply
/// examined on the next full pass, and a file deleted before its shard was visited was already
/// unreachable".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Which role tree is being walked.
    pub role: Role,
    /// The shard index inside it.
    pub shard: u8,
    /// The last name examined in that shard, or `None` at its start.
    pub after: Option<LeafName>,
}

impl Cursor {
    /// The start of a pass: `GEN` shard zero.
    pub const START: Cursor = Cursor { role: Role::Gen, shard: 0, after: None };
}

/// One shard directory tree, as the collector needs it.
pub trait ShardDirectory {
    /// What a directory operation can fail with.
    type Error;

    /// The next 8.3 name in `shard` strictly after `after`, in name order.
    ///
    /// **An absent shard directory is an empty shard**, and must yield `Ok(None)` rather than an
    /// error: shards are created on first use, so most of them do not exist.
    fn next_leaf(&mut self, role: Role, shard: u8, after: Option<&LeafName>) -> Result<Option<LeafName>, Self::Error>;

    /// Deletes a leaf if it is present, and reports success when it was already gone.
    ///
    /// Idempotence is what makes §9's interrupted pair deletion "harmless orphan cleanup": whichever
    /// of the two files a cut left behind, the next pass deletes the remainder and the one already
    /// gone costs nothing.
    fn delete(&mut self, role: Role, leaf: &LeafName) -> Result<(), Self::Error>;
}

/// What one bounded collection step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The generation was unreachable and its `GEN`/`WORK` pair was deleted.
    Deleted {
        /// The generation.
        generation: GenerationId,
    },
    /// The generation is still named, and by what.
    Kept {
        /// The generation.
        generation: GenerationId,
        /// Why it was kept.
        class: Class,
    },
    /// A name in a shard is not one §3 produces.
    ///
    /// §9 says GC "stops on an unknown record or path". Read as *halt the pass*, one stray file
    /// under one shard would make every generation on the card permanently uncollectable — a
    /// directory a human can write into is precisely where a stray file comes from, and §12.1
    /// already establishes that "a stray file there is not corruption". So this is the narrower
    /// reading, amended into §9 by this change: the unknown entry is left exactly where it is,
    /// never opened and never deleted, it is reported, and the pass continues past it. Only that
    /// entry is exempt from collection.
    Unknown {
        /// The name, as it was found.
        leaf: LeafName,
    },
    /// The shard held nothing further; the cursor has moved on.
    ShardComplete {
        /// Which role tree.
        role: Role,
        /// Which shard.
        shard: u8,
    },
    /// Both role trees have been walked once. The next step starts a new pass.
    PassComplete,
}

/// The incremental collector (§9).
#[derive(Debug, Clone, Copy)]
pub struct Collector {
    cursor: Cursor,
    passes: u32,
}

impl Default for Collector {
    fn default() -> Self {
        Collector::new()
    }
}

impl Collector {
    /// A collector at the start of its first pass.
    pub const fn new() -> Self {
        Collector { cursor: Cursor::START, passes: 0 }
    }

    /// Where the pass has reached.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// How many complete passes have finished.
    pub fn passes(&self) -> u32 {
        self.passes
    }

    /// Runs one bounded step: examine one leaf, decide it, and delete at most that one generation.
    ///
    /// Reachability is recomputed inside the step, immediately before the deletion it authorizes,
    /// which is §9's requirement and the reason nothing here caches a reachable set.
    pub fn step<S, D>(
        &mut self,
        index: &RamIndex,
        leases: &LeaseTable,
        source: &mut S,
        directory: &mut D,
        scratch: &mut [u8; resolution::MAX_BODY_LEN],
    ) -> Result<Step, GcError<S::Error, D::Error>>
    where
        S: ReachabilitySource,
        D: ShardDirectory,
    {
        let Cursor { role, shard, after } = self.cursor;
        let next = directory.next_leaf(role, shard, after.as_ref()).map_err(GcError::Directory)?;
        let Some(leaf) = next else {
            self.advance_shard();
            return Ok(if self.cursor == Cursor::START && self.passes > 0 {
                Step::PassComplete
            } else {
                Step::ShardComplete { role, shard }
            });
        };
        self.cursor.after = Some(leaf);

        // §9's "stops on an unknown record or path", read as narrowly as it can be: the cursor has
        // already moved past this name, so the entry is skipped rather than the pass halted. The
        // file is not opened, not deleted and not classified; only it is exempt. Halting instead
        // would let one stray file in one shard make every generation on the card permanently
        // uncollectable, and `IMPORT` already establishes that a stray file is not corruption.
        let Some(generation) = leaf.generation() else {
            return Ok(Step::Unknown { leaf });
        };
        let class = classify(index, leases, generation, source, scratch).map_err(GcError::Source)?;
        if !class.is_collectable() {
            return Ok(Step::Kept { generation, class });
        }

        // §9: "Deleting an unreachable GEN/WORK pair may be interrupted at either file; both
        // orderings recover as harmless orphan cleanup because no catalog fact points to it."
        directory.delete(Role::Gen, &leaf).map_err(GcError::Directory)?;
        directory.delete(Role::Work, &leaf).map_err(GcError::Directory)?;
        Ok(Step::Deleted { generation })
    }

    fn advance_shard(&mut self) {
        self.cursor.after = None;
        if (self.cursor.shard as usize) + 1 < SHARD_COUNT {
            self.cursor.shard += 1;
            return;
        }
        self.cursor.shard = 0;
        match self.cursor.role {
            Role::Gen => self.cursor.role = Role::Work,
            Role::Work => {
                self.cursor.role = Role::Gen;
                self.passes = self.passes.saturating_add(1);
            }
        }
    }
}

/// Why a collection step stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcError<S, D> {
    /// A bounded reachability read failed.
    Source(S),
    /// A directory enumeration or deletion failed.
    Directory(D),
}

#[cfg(test)]
mod tests {
    use super::super::entries::{CatalogHead, DraftPartState, RetainedPrevious};
    use super::super::model::CatalogModel;
    use super::super::resolution::ResolutionEntry;
    use super::super::samples;
    use super::*;
    use obc_link::ids::{DraftPartRef, SessionId};
    use std::collections::{BTreeMap, BTreeSet};
    use std::vec::Vec;

    /// A card: the resolution tables it holds, and the head fields its checkpoint stores.
    #[derive(Default)]
    struct Card {
        resolutions: BTreeMap<u64, Vec<u8>>,
        unreadable: BTreeSet<u64>,
        head_fields: BTreeMap<(u16, u64), CardHeadFields>,
        reads: usize,
    }

    impl Card {
        fn table(&mut self, generation: u64, entries: &[ResolutionEntry]) {
            let mut body = std::vec![0u8; resolution::MAX_BODY_LEN];
            let len = resolution::encode(entries, &mut body).expect("a table encodes");
            body.truncate(len);
            self.resolutions.insert(generation, body);
        }

        fn manifest(&mut self, head: &CatalogHead) {
            self.head_fields.insert((head.key.kind, head.key.id.get()), CardHeadFields::of(head));
        }
    }

    impl ReachabilitySource for Card {
        type Error = ();

        fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, ()> {
            Ok(self.head_fields.get(&(entry.kind, entry.id.get())).copied().unwrap_or(CardHeadFields {
                envelope_len: 8,
                envelope: [0u8; 96],
                resolution_present: false,
                resolution: GenerationId::ZERO,
            }))
        }

        fn resolution(
            &mut self,
            generation: GenerationId,
            into: &mut [u8; resolution::MAX_BODY_LEN],
        ) -> Result<Option<usize>, ()> {
            self.reads += 1;
            if self.unreadable.contains(&generation.get()) {
                return Ok(None);
            }
            let Some(body) = self.resolutions.get(&generation.get()) else { return Ok(None) };
            into[..body.len()].copy_from_slice(body);
            Ok(Some(body.len()))
        }
    }

    /// A shard tree. Absent shards are simply absent — most of them are, on a young card.
    #[derive(Default)]
    struct Tree {
        files: BTreeSet<(Role, LeafName)>,
        fail_delete_at: Option<usize>,
        deletes: usize,
    }

    impl Tree {
        fn add(&mut self, generation: u64) {
            let leaf = LeafName::of(GenerationId::new(generation));
            self.files.insert((Role::Gen, leaf));
            self.files.insert((Role::Work, leaf));
        }

        fn generations(&self, role: Role) -> BTreeSet<u64> {
            self.files
                .iter()
                .filter(|(held, _)| *held == role)
                .filter_map(|(_, leaf)| leaf.generation().map(|id| id.get()))
                .collect()
        }
    }

    impl ShardDirectory for Tree {
        type Error = &'static str;

        fn next_leaf(
            &mut self,
            role: Role,
            shard: u8,
            after: Option<&LeafName>,
        ) -> Result<Option<LeafName>, &'static str> {
            Ok(self
                .files
                .iter()
                .filter(|(held, leaf)| *held == role && leaf.shard.index() == shard)
                .map(|(_, leaf)| *leaf)
                .find(|leaf| after.is_none_or(|previous| leaf > previous)))
        }

        fn delete(&mut self, role: Role, leaf: &LeafName) -> Result<(), &'static str> {
            self.deletes += 1;
            if self.fail_delete_at == Some(self.deletes) {
                return Err("injected");
            }
            self.files.remove(&(role, *leaf));
            Ok(())
        }
    }

    fn index_with_head() -> (std::boxed::Box<RamIndex>, std::boxed::Box<CatalogModel>) {
        let mut model = CatalogModel::initial(samples::STORE, 4);
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        let index = RamIndex::project(&model);
        (index, model)
    }

    /// The lease table §9 makes a RAM ownership fact, empty: most of these cases hold none, and a
    /// pass that consulted a stale one would keep bytes nothing is reading.
    const NO_LEASES: LeaseTable = LeaseTable::new();

    fn scratch() -> std::boxed::Box<[u8; resolution::MAX_BODY_LEN]> {
        std::boxed::Box::new([0u8; resolution::MAX_BODY_LEN])
    }

    /// §9's list, source by source. Each one is a reason on its own, and a generation none of them
    /// names is an orphan.
    #[test]
    fn every_reference_section_9_lists_keeps_a_generation() {
        let (mut index, _model) = index_with_head();
        let mut card = Card::default();
        let mut scratch = scratch();
        let mut leases = LeaseTable::new();
        let mut class = |index: &RamIndex, leases: &LeaseTable, card: &mut Card, generation: u64| {
            classify(index, leases, GenerationId::new(generation), card, &mut scratch).unwrap()
        };

        // The published head's own generation. `samples::head` uses generation 42.
        assert_eq!(
            class(&index, &leases, &mut card, 42),
            Class::Referenced(Reference::Head { kind: 1, id: LogicalObjectId::new(7) }),
        );
        // Nothing names 900.
        assert_eq!(class(&index, &leases, &mut card, 900), Class::Orphan);

        // A retained-previous entry.
        let _ = index.retained.push(samples::retained(900));
        assert_eq!(class(&index, &leases, &mut card, 900), Class::Referenced(Reference::Retained));
        index.retained.clear();

        // A live lease, which nothing on the card records until the head is displaced.
        let lease = leases.pin(1, SessionId::new(1).unwrap(), GenerationId::new(900)).unwrap();
        assert_eq!(class(&index, &leases, &mut card, 900), Class::Referenced(Reference::Lease));
        leases.release(lease, &[]);
        assert_eq!(class(&index, &leases, &mut card, 900), Class::Orphan);

        // The open draft parent's manifest and its reserved resolution generation.
        let mut parent = samples::parent();
        parent.manifest_generation = GenerationId::new(900);
        parent.resolution = GenerationId::new(901);
        index.draft_parent = Some(parent);
        assert_eq!(class(&index, &leases, &mut card, 900), Class::Referenced(Reference::DraftParent));
        assert_eq!(class(&index, &leases, &mut card, 901), Class::Referenced(Reference::DraftParent));

        // A sealed part of it. `samples::part` uses generation 91.
        let mut part = samples::part(1);
        part.state = DraftPartState::Sealed;
        let _ = index.draft_parts.push(part);
        assert_eq!(class(&index, &leases, &mut card, 91), Class::Referenced(Reference::DraftPart));
        index.draft_parent = None;
        index.draft_parts.clear();

        // The recording ride's prospective generation, which is 77.
        index.ride = Some(samples::ride());
        assert_eq!(class(&index, &leases, &mut card, 77), Class::Referenced(Reference::ActiveRide));
        index.ride = None;

        // The update handoff's package, which is 31.
        index.handoff = Some(samples::handoff_ref(4, super::super::handoff::HandoffPhase::Armed));
        assert_eq!(class(&index, &leases, &mut card, 31), Class::Referenced(Reference::UpdateHandoff));
        index.handoff = None;

        // An active claim's reserved generation is resumable work, not an orphan.
        let mut row = samples::active(samples::OP_B);
        row.generation = GenerationId::new(902);
        let _ = index.actives.push(row);
        assert_eq!(class(&index, &leases, &mut card, 902), Class::ResumableWork);
    }

    /// §9's "active operations and WORK records" are one root, not two: §7 reserves the generation
    /// in the catalog journal before either file exists, so an active row's reserved generation is
    /// the only thing a WORK record can be named by.
    ///
    /// The half that matters operationally is the negative one — a `WORK` leaf no active row
    /// reserves is an orphan — because the collector deletes a `WORK` leaf as the pair of a `GEN`
    /// leaf it classified, and would strand it otherwise.
    #[test]
    fn a_work_record_is_rooted_by_its_active_rows_reserved_generation() {
        let (mut index, _model) = index_with_head();
        let mut card = Card::default();
        let mut scratch = scratch();

        // A claim that reserved generation 902 keeps both of its files.
        let mut row = samples::active(samples::OP_B);
        row.generation = GenerationId::new(902);
        assert_ne!(row.flags & ActiveOperation::FLAG_GENERATION_RESERVED, 0);
        let _ = index.actives.push(row);
        assert_eq!(
            classify(&index, &NO_LEASES, GenerationId::new(902), &mut card, &mut scratch).unwrap(),
            Class::ResumableWork,
        );

        // A row that reserved nothing roots nothing, even carrying the same numeric generation.
        index.actives.clear();
        let mut without = samples::active(samples::OP_B);
        without.generation = GenerationId::new(902);
        without.flags &= !ActiveOperation::FLAG_GENERATION_RESERVED;
        let _ = index.actives.push(without);
        assert_eq!(
            classify(&index, &NO_LEASES, GenerationId::new(902), &mut card, &mut scratch).unwrap(),
            Class::Orphan
        );

        // And the collector removes both leaves of that orphan, WORK included, without ever having
        // classified the WORK leaf on its own.
        let mut tree = Tree::default();
        tree.add(902);
        let mut collector = Collector::new();
        let mut steps = 0;
        while collector.passes() == 0 {
            steps += 1;
            assert!(steps < 4_000);
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
        }
        assert!(tree.generations(Role::Gen).is_empty());
        assert!(tree.generations(Role::Work).is_empty(), "the WORK leaf of an orphan was stranded");
    }

    /// §9 walks the resolution table, not the manifest payload — one bounded read per manifest head.
    #[test]
    fn a_manifest_reaches_its_children_through_the_resolution_table() {
        let mut model = CatalogModel::initial(samples::STORE, 4);
        let manifest = samples::manifest_head(3, 92);
        model.apply(&samples::claim(1, 1, 0, samples::OP_PARENT, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_PARENT, 1, manifest)).unwrap();
        let index = RamIndex::project(&model);

        let mut card = Card::default();
        card.manifest(&manifest);
        let children = [
            ResolutionEntry { part_ref: DraftPartRef::new([0x11; 16]), generation: GenerationId::new(500) },
            ResolutionEntry { part_ref: DraftPartRef::new([0x22; 16]), generation: GenerationId::new(501) },
        ];
        card.table(92, &children);
        let mut scratch = scratch();

        for child in [500u64, 501] {
            assert_eq!(
                classify(&index, &NO_LEASES, GenerationId::new(child), &mut card, &mut scratch).unwrap(),
                Class::Referenced(Reference::ManifestChild { manifest: manifest.generation }),
            );
        }
        // The resolution generation itself is reachable from the head's own field.
        assert_eq!(
            classify(&index, &NO_LEASES, GenerationId::new(92), &mut card, &mut scratch).unwrap(),
            Class::Referenced(Reference::ResolutionTable { manifest: manifest.generation }),
        );
        // A generation the table does not name is an orphan.
        assert_eq!(
            classify(&index, &NO_LEASES, GenerationId::new(502), &mut card, &mut scratch).unwrap(),
            Class::Orphan
        );
        // §9 costs a pass at most eight bounded 776-byte reads: one manifest head, one read each.
        assert!(card.reads <= 8 * 4, "a classification read the table {} times", card.reads);
    }

    /// §9: "If a resolution generation cannot be read or its count and length disagree, every
    /// generation that manifest could name is treated as reachable." Both failures, and the
    /// consequence — nothing is collectable while the evidence is torn.
    #[test]
    fn unreadable_or_malformed_resolution_evidence_never_orphans_anything() {
        let mut model = CatalogModel::initial(samples::STORE, 4);
        let manifest = samples::manifest_head(3, 92);
        model.apply(&samples::claim(1, 1, 0, samples::OP_PARENT, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_PARENT, 1, manifest)).unwrap();
        let index = RamIndex::project(&model);
        let mut scratch = scratch();

        // Absent: the generation file is gone.
        let mut card = Card::default();
        card.manifest(&manifest);
        assert_eq!(
            classify(&index, &NO_LEASES, GenerationId::new(500), &mut card, &mut scratch).unwrap(),
            Class::Unresolved { manifest: manifest.generation },
        );

        // Present but unreadable.
        card.table(
            92,
            &[ResolutionEntry { part_ref: DraftPartRef::new([0x11; 16]), generation: GenerationId::new(500) }],
        );
        card.unreadable.insert(92);
        assert_eq!(
            classify(&index, &NO_LEASES, GenerationId::new(700), &mut card, &mut scratch).unwrap(),
            Class::Unresolved { manifest: manifest.generation },
        );

        // Present and readable, but its count and length disagree — §8's two checks are the whole
        // validity test a torn one-shot write fails.
        card.unreadable.clear();
        let torn = card.resolutions.get_mut(&92).unwrap();
        torn.truncate(torn.len() - 1);
        assert_eq!(
            classify(&index, &NO_LEASES, GenerationId::new(700), &mut card, &mut scratch).unwrap(),
            Class::Unresolved { manifest: manifest.generation },
        );

        // And the collector deletes nothing at all while that is true.
        let mut tree = Tree::default();
        tree.add(700);
        let mut collector = Collector::new();
        let mut steps = 0;
        while collector.passes() == 0 && steps < 4_000 {
            let step = collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
            assert!(!matches!(step, Step::Deleted { .. }), "torn evidence authorized a deletion");
            steps += 1;
        }
        assert!(tree.generations(Role::Gen).contains(&700));
    }

    /// The property: over a randomized store, a complete pass deletes exactly the generations
    /// nothing names, and never one the reference model reaches.
    #[test]
    fn a_complete_pass_deletes_exactly_the_unreachable_generations() {
        let mut rng = 0x5EED_1234_ABCD_0001u64;
        let mut next = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };

        for round in 0..8u64 {
            let mut model = CatalogModel::initial(samples::STORE, 4);
            let mut expected_kept: BTreeSet<u64> = BTreeSet::new();
            let mut tree = Tree::default();
            let mut card = Card::default();

            // A handful of ordinary published heads, each with its own generation.
            let publications = 1 + next() % 6;
            for step in 1..=publications {
                let mut operation = samples::OP_A;
                operation[0] = step as u8;
                operation[1] = round as u8;
                let mut head = samples::head(1, step);
                head.generation = GenerationId::new(1_000 + round * 100 + step);
                model.apply(&samples::claim(1, step * 2 - 1, 0, operation, model.next_generation + 1)).unwrap();
                model.apply(&samples::publish(1, step * 2, 0, operation, step, head)).unwrap();
                expected_kept.insert(head.generation.get());
                tree.add(head.generation.get());
            }

            // One manifest head with children, sometimes.
            if next() % 2 == 0 {
                let mut manifest = samples::manifest_head(50 + round, 2_000 + round * 100);
                manifest.generation = GenerationId::new(3_000 + round);
                let mut operation = samples::OP_PARENT;
                operation[1] = round as u8;
                let sequence = model.through_sequence + 1;
                model.apply(&samples::claim(1, sequence, 0, operation, model.next_generation + 1)).unwrap();
                model.apply(&samples::publish(1, sequence + 1, 0, operation, publications + 1, manifest)).unwrap();
                card.manifest(&manifest);
                let children: Vec<ResolutionEntry> = (0..3u64)
                    .map(|index| {
                        let mut bytes = [0u8; 16];
                        bytes[0] = index as u8;
                        ResolutionEntry {
                            part_ref: DraftPartRef::new(bytes),
                            generation: GenerationId::new(4_000 + round * 10 + index),
                        }
                    })
                    .collect();
                card.table(manifest.resolution.get(), &children);
                expected_kept.insert(manifest.generation.get());
                expected_kept.insert(manifest.resolution.get());
                tree.add(manifest.generation.get());
                tree.add(manifest.resolution.get());
                for child in &children {
                    expected_kept.insert(child.generation.get());
                    tree.add(child.generation.get());
                }
            }

            // A retained entry and a lease, whose reasons are of very different kinds.
            let mut index = RamIndex::project(&model);
            if next() % 2 == 0 {
                let mut entry = samples::retained(5_000 + round);
                entry.reasons = RetainedPrevious::REASON_UPDATE_ROLLBACK;
                entry.lease_count = 0;
                let _ = index.retained.push(entry);
                expected_kept.insert(entry.generation.get());
                tree.add(entry.generation.get());
            }
            let leased = 6_000 + round;
            let mut leases = LeaseTable::new();
            leases.pin(1, SessionId::new(1).unwrap(), GenerationId::new(leased)).unwrap();
            expected_kept.insert(leased);
            tree.add(leased);

            // And a scatter of generations nothing names.
            let mut expected_gone: BTreeSet<u64> = BTreeSet::new();
            for _ in 0..(3 + next() % 8) {
                let orphan = 7_000 + next() % 5_000;
                if expected_kept.contains(&orphan) {
                    continue;
                }
                expected_gone.insert(orphan);
                tree.add(orphan);
            }

            let mut scratch = scratch();
            let mut collector = Collector::new();
            let mut deleted: BTreeSet<u64> = BTreeSet::new();
            let mut steps = 0;
            while collector.passes() == 0 {
                steps += 1;
                assert!(steps < 4_000, "the pass did not terminate");
                match collector.step(&index, &leases, &mut card, &mut tree, &mut scratch).unwrap() {
                    Step::Deleted { generation } => {
                        deleted.insert(generation.get());
                    }
                    Step::Kept { generation, class } => {
                        assert!(
                            expected_kept.contains(&generation.get()),
                            "round {round}: {} was kept as {class:?} but nothing should name it",
                            generation.get(),
                        );
                    }
                    _ => {}
                }
            }

            assert_eq!(deleted, expected_gone, "round {round}: the wrong set was collected");
            assert_eq!(tree.generations(Role::Gen), expected_kept, "round {round}: GEN");
            assert_eq!(tree.generations(Role::Work), expected_kept, "round {round}: WORK");
        }
    }

    /// §9: "One GC step visits at most one shard and deletes at most one generation."
    #[test]
    fn one_step_deletes_at_most_one_generation() {
        let (index, _model) = index_with_head();
        let mut card = Card::default();
        let mut tree = Tree::default();
        // Three orphans in the same shard: low byte 0x00 for all of them.
        for high in 1..=3u64 {
            tree.add(high << 8);
        }
        let mut scratch = scratch();
        let mut collector = Collector::new();

        for expected in 1..=3u64 {
            let step = collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
            assert_eq!(step, Step::Deleted { generation: GenerationId::new(expected << 8) });
        }
        // And the shard is then done; the cursor moves on rather than looping.
        assert_eq!(
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap(),
            Step::ShardComplete { role: Role::Gen, shard: 0 },
        );
        assert_eq!(collector.cursor().shard, 1);
    }

    /// Absent shards are empty shards. A store whose tree was created lazily walks cleanly.
    #[test]
    fn an_absent_shard_is_an_empty_shard() {
        let (index, _model) = index_with_head();
        let mut card = Card::default();
        // No files at all: every shard of both roles is absent.
        let mut tree = Tree::default();
        let mut scratch = scratch();
        let mut collector = Collector::new();
        let mut steps = 0;
        while collector.passes() == 0 {
            steps += 1;
            assert!(steps <= 2 * SHARD_COUNT + 1, "a lazily created tree did not walk cleanly");
            let step = collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
            assert!(matches!(step, Step::ShardComplete { .. } | Step::PassComplete), "{step:?}");
        }
        assert_eq!(steps, 2 * SHARD_COUNT, "every shard of both roles is visited exactly once");
    }

    /// A name that is not one §3 produces is never opened and never deleted, and — the half the
    /// spec sentence had to be narrowed to say — the pass continues past it.
    ///
    /// The stray sits between two collectable orphans of the same shard, so a pass that halted on it
    /// would leave the second one uncollected. That is the difference the §9 amendment makes, and
    /// asserting only the first half would not have caught it.
    #[test]
    fn an_unknown_name_is_left_where_it_is_and_the_pass_continues_past_it() {
        let (index, _model) = index_with_head();
        let mut card = Card::default();
        let mut tree = Tree::default();
        // All in shard 0, and `ZZZZZZZZ.ZZZ` sorts after both base-36 names.
        tree.add(0x100);
        tree.add(0x200);
        let stray = LeafName::parse("00", "ZZZZZZZZ.ZZZ").unwrap();
        tree.files.insert((Role::Gen, stray));
        let mut scratch = scratch();
        let mut collector = Collector::new();

        assert_eq!(
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap(),
            Step::Deleted { generation: GenerationId::new(0x100) },
        );
        assert_eq!(
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap(),
            Step::Deleted { generation: GenerationId::new(0x200) },
        );
        let deletes_before = tree.deletes;
        assert_eq!(
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap(),
            Step::Unknown { leaf: stray }
        );
        assert_eq!(tree.deletes, deletes_before, "an unknown name was deleted");
        assert!(tree.files.contains(&(Role::Gen, stray)), "an unknown name was removed");

        // And the pass runs on rather than stopping here: the cursor leaves the shard normally and
        // a later orphan in a later shard is still collected.
        tree.add(0x105);
        assert_eq!(
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap(),
            Step::ShardComplete { role: Role::Gen, shard: 0 },
        );
        let mut steps = 0;
        while collector.passes() == 0 {
            steps += 1;
            assert!(steps < 4_000, "the pass halted on the unknown name");
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
        }
        assert!(!tree.generations(Role::Gen).contains(&0x105), "the orphan behind the stray was never collected");
        assert!(tree.files.contains(&(Role::Gen, stray)), "the stray survived the whole pass, untouched");
    }

    /// §9's interrupted pair deletion, at both boundaries: whichever file the cut left behind, the
    /// next pass finishes the job and nothing else is disturbed.
    #[test]
    fn an_interrupted_pair_deletion_recovers_as_harmless_orphan_cleanup() {
        for cut_at in [1usize, 2] {
            let (index, _model) = index_with_head();
            let mut card = Card::default();
            let mut tree = Tree::default();
            tree.add(0x300); // an orphan, in shard 0
            tree.add(42); // the published head's generation, in shard 42
            tree.fail_delete_at = Some(cut_at);
            let mut scratch = scratch();
            let mut collector = Collector::new();

            assert_eq!(
                collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch),
                Err(GcError::Directory("injected")),
                "cut at delete {cut_at}",
            );
            // Whichever half survived, the catalog is untouched and the head's files are intact.
            assert!(tree.generations(Role::Gen).contains(&42));
            assert!(tree.generations(Role::Work).contains(&42));

            // The next mount restarts the pass from shard zero and finishes the deletion.
            tree.fail_delete_at = None;
            let mut collector = Collector::new();
            let mut steps = 0;
            while collector.passes() == 0 {
                steps += 1;
                assert!(steps < 4_000);
                collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
            }
            assert!(!tree.generations(Role::Gen).contains(&0x300), "cut at {cut_at}: the GEN leaf survived");
            assert!(!tree.generations(Role::Work).contains(&0x300), "cut at {cut_at}: the WORK leaf survived");
            assert_eq!(tree.generations(Role::Gen), BTreeSet::from([42]));
        }
    }

    /// A file created after its shard was visited is examined on the next pass, which is §9's
    /// snapshot argument stated as a test rather than as a comment.
    #[test]
    fn a_file_created_behind_the_cursor_is_collected_on_the_next_pass() {
        let (index, _model) = index_with_head();
        let mut card = Card::default();
        let mut tree = Tree::default();
        let mut scratch = scratch();
        let mut collector = Collector::new();

        // Walk past shard 3 of both role trees — the pass visits `GEN` and then `WORK`, so a file
        // is only behind the cursor once both have been passed.
        for _ in 0..(SHARD_COUNT + 4) {
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
        }
        assert_eq!((collector.cursor().role, collector.cursor().shard), (Role::Work, 4));
        tree.add(0x100 | 3); // shard 3, behind the cursor in both trees

        while collector.passes() == 0 {
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
        }
        assert!(tree.generations(Role::Gen).contains(&0x103), "the pass collected behind its own cursor");

        // The next pass sees it.
        while collector.passes() == 1 {
            collector.step(&index, &NO_LEASES, &mut card, &mut tree, &mut scratch).unwrap();
        }
        assert!(tree.generations(Role::Gen).is_empty());
    }
}
