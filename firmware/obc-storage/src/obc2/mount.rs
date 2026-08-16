//! Mount classification, initialization shape, and sideload staging (`OBC2_Storage_Format.md` §12
//! and §12.1).
//!
//! §12 is "the sole authority for the store's mount classification", and the wire contract's
//! `GetDeviceStatus` enum "reports these values verbatim and invents no store state of its own". So
//! this module is that decision tree and nothing else: it reads no bytes, opens no file and repairs
//! nothing. It is handed what a mount observed — the §1.1 volume verdict, §6.3's checkpoint
//! decision, whether an `INIT` witness validated, and the shape of `/OBC2` — and returns what to do.
//!
//! ## Lazy shard directories
//!
//! §12's creation order originally created `GEN` and `WORK` with all 512 of their shard directories
//! at initialization. On the shipped media that measured **73.5 seconds**, 98% of a 75-second first
//! boot, because the cost is per-directory: each `make_dir` allocates a cluster, writes its `.`/`..`
//! entries, updates the FAT and rescans a growing parent. The owner decision of 2026-08-16 is
//! **lazy shard creation**: a shard is created by the one bounded `make_dir` at the admission that
//! first needs it, and first boot drops to the fixed files and their zero-fill — about 1.7 seconds.
//!
//! Two consequences run through this module. [`CREATION_ORDER`] no longer contains shards, so the
//! pre-birth prefix is seven files rather than a tree; and an absent shard is everywhere equivalent
//! to an empty one, which §12's reuse rule already made true of a present-but-empty directory.
//! [`shard_to_create`] is the admission-time obligation that replaces the eager pass.
//!
//! ## What class 4 is, and why it is not decided here
//!
//! §12 makes "mounted with degraded entries" **dynamic**: "a store mounts `3` and becomes `4` at the
//! first such pin. It stays fully writable, every other entry is served, and the class does not
//! return to `3` within one mount." So [`classify`] never returns 4 — it cannot, because no pin has
//! happened yet — and [`MountState`] is what carries the class forward through a mount and moves it
//! when a lazy pin fails. Class 6 is the other half of that distinction and is durable: it comes
//! from the checkpoint header's own recovery-degraded bit.

use obc_link::ids::{GenerationId, StoreId};

use super::geometry::Unsupported;
use super::limits::{CHECKPOINT_FILE_LEN, INITIALIZATION_ZERO_FILL, JOURNAL_FILE_LEN, RIDE_FILE_LEN, SLOT_FILE_LEN};
use super::names::{Role, ShardName};
use super::recovery::{Decision, FailClosed, ReadOnly};

/// §12's mount classification, reported verbatim by the wire contract's `GetDeviceStatus`.
///
/// Value `0`, no card, is deliberately absent: §12 puts it in the link layer, "with no medium there
/// is nothing to classify".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum MountClass {
    /// A volume precondition of §1.1 failed. Nothing is written and `/OBC2` is never looked for.
    UnsupportedFilesystem = 1,
    /// No valid checkpoint exists yet. Transient; no traffic is served and no `StoreId` advertised.
    Initializing = 2,
    /// A valid checkpoint is mounted and the bounded recovery suffix is complete.
    Mounted = 3,
    /// As `Mounted`, plus at least one catalog entry has failed its lazy pin since this mount.
    MountedDegradedEntries = 4,
    /// A lost gated metadata record, a lost single-copy FAT structure, an unknown `/OBC2` shape, or
    /// equal-sequence differing records. Evidence preserved, nothing repaired, no mutation admitted.
    RecoveryFailed = 5,
    /// The catalog is intact but a store-wide condition needs explicit recovery before mutation.
    MountedStoreDegraded = 6,
}

/// Why a mount failed closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// §6.3's fail-closed rules over the checkpoints and the journal.
    Journal(FailClosed),
    /// §1.1: a lost boot sector, FSInfo sector or directory sector "destroys file locations for the
    /// whole store". Not a gated-record fault, and never silently reinitialized.
    LostFatStructure,
    /// §12: "any other nonempty or unknown OBC2 shape".
    UnknownShape,
}

/// What a mount decided to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// §1.1's volume preconditions failed. Nothing is written.
    Unsupported(Unsupported),
    /// A fresh card: `/OBC2` is absent, or holds nothing but `IMPORT` and staged files.
    Initialize,
    /// A valid `INIT` witness with only its own ordered preallocation prefix: resume, preserving
    /// that unadvertised `StoreId`.
    ResumeInitialization {
        /// The witness's `StoreId`, which has never escaped `CardStore`.
        store: StoreId,
    },
    /// An exact ungated pre-birth prefix and no witness: delete those files and restart with a new
    /// `StoreId`.
    RestartPreBirth {
        /// How many files of the creation order are present and must be removed.
        files: usize,
    },
    /// Mount the selected checkpoint and replay that many leading journal slots.
    Mount {
        /// Which checkpoint file, `0` or `1`.
        checkpoint: usize,
        /// How many leading journal slots form the contiguous valid suffix.
        replay: usize,
        /// §5.2's exhausted monotonic space, when one ran out. Replay is unchanged; no mutation is
        /// admitted until an explicit reset.
        exhausted: Option<ReadOnly>,
        /// §12's class 6: the checkpoint header's durable recovery-degraded bit.
        store_degraded: bool,
    },
    /// Mount recovery-failed and read-only, preserving all evidence.
    RecoveryFailed(Fault),
}

impl Outcome {
    /// The class §12's table reports for this outcome.
    ///
    /// Class 4 is never produced here: it is dynamic and belongs to [`MountState`].
    pub fn class(&self) -> MountClass {
        match self {
            Outcome::Unsupported(_) => MountClass::UnsupportedFilesystem,
            Outcome::Initialize | Outcome::ResumeInitialization { .. } | Outcome::RestartPreBirth { .. } => {
                MountClass::Initializing
            }
            Outcome::Mount { store_degraded: true, .. } => MountClass::MountedStoreDegraded,
            Outcome::Mount { .. } => MountClass::Mounted,
            Outcome::RecoveryFailed(_) => MountClass::RecoveryFailed,
        }
    }

    /// Whether this outcome admits any write to the card at all.
    ///
    /// §12: an unsupported volume is never written to, and a recovery-failed store "is never
    /// silently reinitialized". An exhausted monotonic space is read-only for the same reason with a
    /// different cause.
    pub fn admits_mutation(&self) -> bool {
        match self {
            Outcome::Mount { exhausted, store_degraded, .. } => exhausted.is_none() && !store_degraded,
            Outcome::Initialize | Outcome::ResumeInitialization { .. } | Outcome::RestartPreBirth { .. } => true,
            Outcome::Unsupported(_) | Outcome::RecoveryFailed(_) => false,
        }
    }
}

/// One fixed OBC2 file, in the order §12 creates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedFile {
    /// Its uppercase 8.3 name.
    pub name: &'static str,
    /// The length initialization gives it.
    pub len: u32,
}

/// §12's file creation order, **amended for lazy shard directories**.
///
/// The directories — `OBC2`, then `GEN`, `WORK` and `IMPORT` — are created around these files and
/// are exempt from every shape judgement, because §12 reuses a present empty directory rather than
/// removing it and the shard leaves are now created on first use. What remains ordered, and what a
/// pre-birth prefix is judged against, is exactly these seven files in exactly this order.
pub const CREATION_ORDER: [FixedFile; 7] = [
    FixedFile { name: "INIT.REC", len: SLOT_FILE_LEN as u32 },
    FixedFile { name: "COMMIT.JNL", len: JOURNAL_FILE_LEN as u32 },
    FixedFile { name: "ARM0.HND", len: SLOT_FILE_LEN as u32 },
    FixedFile { name: "ARM1.HND", len: SLOT_FILE_LEN as u32 },
    FixedFile { name: "RIDE.ACT", len: RIDE_FILE_LEN as u32 },
    FixedFile { name: "CAT0.CHK", len: CHECKPOINT_FILE_LEN as u32 },
    FixedFile { name: "CAT1.CHK", len: CHECKPOINT_FILE_LEN as u32 },
];

/// The directories initialization creates, in order. Four, not 516: the 512 shard leaves are lazy.
pub const CREATION_DIRECTORIES: [&str; 4] = ["OBC2", "GEN", "WORK", "IMPORT"];

/// One `/OBC2` directory entry a mount observed, in FAT physical order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The 8.3 name, uppercase.
    pub name: &'a str,
    /// The recorded length.
    pub len: u32,
}

/// What a mount observed of an existing `/OBC2`.
#[derive(Debug, Clone, Copy)]
pub struct StoreShape<'a> {
    /// §6.3's decision over the two checkpoints and all 256 journal slots.
    pub decision: Decision,
    /// The `StoreId` of a valid `INIT.REC` witness, when one validated.
    pub witness: Option<StoreId>,
    /// The OBC2-owned files present directly under `/OBC2`, in FAT physical directory-entry order.
    ///
    /// §12 judges a pre-birth prefix "over the FAT physical directory-entry order — the order the
    /// adapter enumerates, which is the order initialization created them — not over a sorted or
    /// otherwise normalized listing". Directories are not entries here, and neither is anything
    /// under `IMPORT`.
    pub files: &'a [Entry<'a>],
    /// Whether any present slot carries a valid OBC2 gate. A pre-birth prefix has none.
    pub any_valid_gate: bool,
    /// Whether §1.1's single-copy FAT structures are intact.
    pub fat_intact: bool,
    /// The selected checkpoint header's recovery-degraded bit (§5.2 byte 59 bit 0).
    pub store_degraded: bool,
}

/// §12's decision tree, in the order it evaluates.
///
/// `volume` is `Some` when §1.1 refused the volume, which is decided "before it looks for `/OBC2`".
/// `store` is `None` when there is no `/OBC2` directory at all.
pub fn classify(volume: Option<Unsupported>, store: Option<StoreShape<'_>>) -> Outcome {
    // "an unsupported filesystem or no readable FAT volume by section 1.1: mount unsupported and
    // write nothing; this is decided before `/OBC2` is looked for".
    if let Some(reason) = volume {
        return Outcome::Unsupported(reason);
    }
    // "no `OBC2`: initialize".
    let Some(shape) = store else { return Outcome::Initialize };

    // "a lost single-copy FAT structure by section 1.1: mount recovery-failed/read-only". It is
    // checked before the checkpoint decision because a store whose file locations are gone cannot
    // have produced trustworthy observations of them.
    if !shape.fat_intact {
        return Outcome::RecoveryFailed(Fault::LostFatStructure);
    }

    match shape.decision {
        // "a valid checkpoint: mount it, even if a stale INIT record remains, then replay".
        Decision::Mount { checkpoint, replay } => {
            Outcome::Mount { checkpoint, replay, exhausted: None, store_degraded: shape.store_degraded }
        }
        Decision::MountReadOnly { checkpoint, replay, reason } => {
            Outcome::Mount { checkpoint, replay, exhausted: Some(reason), store_degraded: shape.store_degraded }
        }
        Decision::Fail(fault) => Outcome::RecoveryFailed(Fault::Journal(fault)),
        Decision::NoCheckpoint => classify_pre_birth(&shape),
    }
}

/// The three pre-birth cases §12 distinguishes, once no checkpoint is valid.
fn classify_pre_birth(shape: &StoreShape<'_>) -> Outcome {
    // "A card whose `/OBC2` contains nothing but `IMPORT` and staged files is a fresh card": the
    // caller has already excluded `IMPORT` from `files`, so an empty listing is that card.
    if shape.files.is_empty() && shape.witness.is_none() && !shape.any_valid_gate {
        return Outcome::Initialize;
    }
    // "An unknown name, oversize entry, or valid gate is not a pre-birth prefix and fails closed."
    let Prefix::Exact(files) = prefix_verdict(shape.files) else {
        return Outcome::RecoveryFailed(Fault::UnknownShape);
    };
    if shape.any_valid_gate {
        return Outcome::RecoveryFailed(Fault::UnknownShape);
    }
    match shape.witness {
        // "With a valid INIT but no checkpoint, recovery preserves its StoreId, truncates or
        // completes only the same ordered preallocation prefix … and resumes initialization."
        Some(store) => Outcome::ResumeInitialization { store },
        // "no valid checkpoint or INIT but an exact ungated pre-birth prefix: remove it and restart".
        None => Outcome::RestartPreBirth { files },
    }
}

/// Whether a `/OBC2` file listing is an exact prefix of [`CREATION_ORDER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    /// The listing is the first `n` files of the creation order, each at or below its length.
    Exact(usize),
    /// It is not. §12 fails closed on this rather than guessing.
    Foreign,
}

/// §12's pre-birth prefix test.
///
/// "the present files are an exact prefix of the creation order above, every present name has the
/// specified type and at most its specified length, and no present slot has any valid OBC2 gate …
/// The final entry of that prefix may be short or incomplete: a cut during the zero-fill of
/// `INIT.REC` or of any preallocated file leaves a truncated last file, which is a bounded restart
/// case and not a foreign name."
///
/// The order is the FAT physical one, which is the order initialization created them; the caller
/// must not sort.
pub fn prefix_verdict(files: &[Entry<'_>]) -> Prefix {
    if files.len() > CREATION_ORDER.len() {
        return Prefix::Foreign;
    }
    for (entry, expected) in files.iter().zip(CREATION_ORDER.iter()) {
        if !entry.name.eq_ignore_ascii_case(expected.name) || entry.len > expected.len {
            return Prefix::Foreign;
        }
    }
    Prefix::Exact(files.len())
}

/// The shard directory an admission must `make_dir` before it creates this generation's files.
///
/// This is the whole of the lazy-shard obligation: one bounded `make_dir` on a possibly
/// already-present directory, which §12's reuse rule already makes "not an error". It costs about
/// 140 ms the first time a shard is used and nothing afterwards, against the 73.5 seconds the eager
/// tree cost at initialization — and it never runs on the streaming path, only at admission.
pub fn shard_to_create(generation: GenerationId, role: Role) -> (Role, ShardName) {
    (role, super::names::LeafName::of(generation).shard)
}

/// The class a mount carries while it serves traffic, and the one transition §12 makes dynamic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountState {
    class: MountClass,
    degraded_entries: u16,
}

impl MountState {
    /// The state a classified mount starts in.
    pub fn new(outcome: &Outcome) -> Self {
        MountState { class: outcome.class(), degraded_entries: 0 }
    }

    /// The class to report now.
    pub fn class(&self) -> MountClass {
        self.class
    }

    /// How many catalog entries have failed a lazy pin since this mount.
    pub fn degraded_entries(&self) -> u16 {
        self.degraded_entries
    }

    /// Records §12's lazy-pin failure: "A referenced generation is verified lazily, at the first pin
    /// that needs it. A missing or unreadable file discovered then makes that one catalog entry
    /// degraded."
    ///
    /// It moves a `3` to a `4` and never back — and it never touches a `6`, because §12 is explicit
    /// that "a missing generation file never produces class `6`, and a recorded store-wide condition
    /// is never reported as class `4`".
    pub fn note_failed_pin(&mut self) {
        self.degraded_entries = self.degraded_entries.saturating_add(1);
        if self.class == MountClass::Mounted {
            self.class = MountClass::MountedDegradedEntries;
        }
    }
}

/// The three object kinds §12.1 admits from `/OBC2/IMPORT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StagedKind {
    /// `ROUTE*.OBR` → one route Put.
    Route = 1,
    /// `MAP*.OBM` → one standalone-map volume release.
    VolumeManifest = 6,
    /// `UPDATE*.BIN` → one update-package Put.
    UpdatePackage = 7,
}

impl StagedKind {
    /// The `(stem prefix, extension)` pair §12.1's table gives this kind.
    pub const fn pattern(self) -> (&'static str, &'static str) {
        match self {
            StagedKind::Route => ("ROUTE", "OBR"),
            StagedKind::VolumeManifest => ("MAP", "OBM"),
            StagedKind::UpdatePackage => ("UPDATE", "BIN"),
        }
    }

    /// Every importable kind, in the order §12.1's table lists them.
    pub const ALL: [StagedKind; 3] = [StagedKind::Route, StagedKind::VolumeManifest, StagedKind::UpdatePackage];
}

/// A FAT short name exactly as a directory entry holds it: eight stem bytes then three extension
/// bytes, space-padded.
///
/// §12.1 derives an import's identity over these 11 bytes verbatim, "space-padded exactly as the
/// directory entry holds them", so the padding is part of the value rather than a formatting detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShortName(pub [u8; 11]);

impl ShortName {
    /// The eight stem bytes.
    pub fn stem(&self) -> &[u8] {
        &self.0[..8]
    }

    /// The three extension bytes.
    pub fn extension(&self) -> &[u8] {
        &self.0[8..]
    }

    /// The stem with its trailing space padding stripped, which §12.1 makes the display name of an
    /// imported map.
    pub fn stripped_stem(&self) -> &[u8] {
        let mut end = 8;
        while end > 0 && self.0[end - 1] == b' ' {
            end -= 1;
        }
        &self.0[..end]
    }

    /// Builds a short name from `NAME.EXT` text, space-padding both halves.
    pub fn parse(text: &str) -> Option<Self> {
        let (stem, extension) = match text.split_once('.') {
            Some(split) => split,
            None => (text, ""),
        };
        if stem.is_empty() || stem.len() > 8 || extension.len() > 3 {
            return None;
        }
        let mut bytes = [b' '; 11];
        bytes[..stem.len()].copy_from_slice(stem.as_bytes());
        bytes[8..8 + extension.len()].copy_from_slice(extension.as_bytes());
        Some(ShortName(bytes))
    }
}

/// The kind a staged file declares, or `None` for a name §12.1 ignores.
///
/// "The stem begins with the kind's prefix and continues with any legal 8.3 stem characters, so
/// `UPDATE.BIN` and `ROUTE001.OBR` both match. A name must match a prefix and that prefix's
/// extension together; matching one alone does not select a kind."
pub fn classify_staged(name: &ShortName) -> Option<StagedKind> {
    for kind in StagedKind::ALL {
        let (prefix, extension) = kind.pattern();
        if name.stem().starts_with(prefix.as_bytes()) && name.extension() == extension.as_bytes() {
            return Some(kind);
        }
    }
    None
}

/// §12.1's per-mount import bound.
pub const MAX_IMPORTS_PER_MOUNT: usize = 8;

/// What a mount will do with `/OBC2/IMPORT` this time round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    /// The files to import, in FAT physical order, at most eight.
    pub imports: heapless::Vec<(ShortName, StagedKind), MAX_IMPORTS_PER_MOUNT>,
    /// Names that matched no kind. §12.1: "Any other name is ignored — never opened, deleted, or
    /// renamed — and reported through the diagnostic below."
    pub ignored: usize,
    /// Importable files beyond the eighth, left for the next mount.
    pub deferred: usize,
}

/// Plans a mount's imports from `/OBC2/IMPORT`'s listing in FAT physical directory-entry order.
///
/// §12.1: "At most eight staged files are imported per mount, taken in FAT physical
/// directory-entry order — the same order section 12 judges a pre-birth prefix by. Any beyond the
/// eighth are left for the next mount."
pub fn plan_imports(names: impl IntoIterator<Item = ShortName>) -> ImportPlan {
    let mut plan = ImportPlan { imports: heapless::Vec::new(), ignored: 0, deferred: 0 };
    for name in names {
        match classify_staged(&name) {
            None => plan.ignored += 1,
            Some(kind) => {
                if plan.imports.push((name, kind)).is_err() {
                    plan.deferred += 1;
                }
            }
        }
    }
    plan
}

/// The bytes a fresh initialization writes, now that shard directories are lazy.
///
/// The figure itself is unchanged — §13.1's 4,636,672-byte zero-fill is a property of the fixed
/// files, not of the tree — but what it is now *most* of is the story: the eager tree was 73.5 s of
/// a 75 s first boot, and removing it leaves this zero-fill's measured 1.55 s plus a first
/// checkpoint.
pub const INITIALIZATION_BYTES: usize = INITIALIZATION_ZERO_FILL;

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const STORE: StoreId = StoreId::new([0x3c; 16]);

    fn shape<'a>(files: &'a [Entry<'a>], decision: Decision) -> StoreShape<'a> {
        StoreShape { decision, witness: None, files, any_valid_gate: false, fat_intact: true, store_degraded: false }
    }

    fn entry(name: &str, len: u32) -> Entry<'_> {
        Entry { name, len }
    }

    /// §12's table, value by value. The wire contract reports these verbatim, so the discriminants
    /// are contract, not an implementation detail.
    #[test]
    fn the_class_values_are_section_12s_table() {
        assert_eq!(MountClass::UnsupportedFilesystem as u8, 1);
        assert_eq!(MountClass::Initializing as u8, 2);
        assert_eq!(MountClass::Mounted as u8, 3);
        assert_eq!(MountClass::MountedDegradedEntries as u8, 4);
        assert_eq!(MountClass::RecoveryFailed as u8, 5);
        assert_eq!(MountClass::MountedStoreDegraded as u8, 6);
    }

    /// §1.1's verdict is reached before `/OBC2` is looked for at all, and nothing is written.
    #[test]
    fn an_unsupported_volume_is_decided_before_the_store_is_examined() {
        let outcome = classify(Some(Unsupported::DataRegionMisaligned(16_678_913)), None);
        assert_eq!(outcome, Outcome::Unsupported(Unsupported::DataRegionMisaligned(16_678_913)));
        assert_eq!(outcome.class(), MountClass::UnsupportedFilesystem);
        assert!(!outcome.admits_mutation());

        // Even with a perfectly good store on the card, the volume verdict wins.
        let files = [entry("CAT0.CHK", CHECKPOINT_FILE_LEN as u32)];
        let outcome = classify(
            Some(Unsupported::ClusterNotWholePages(20_480)),
            Some(shape(&files, Decision::Mount { checkpoint: 0, replay: 3 })),
        );
        assert_eq!(outcome.class(), MountClass::UnsupportedFilesystem);
    }

    /// The four ordinary rows of §12's mount list.
    #[test]
    fn a_valid_checkpoint_mounts_and_replays() {
        let files = [entry("CAT0.CHK", CHECKPOINT_FILE_LEN as u32)];
        let outcome = classify(None, Some(shape(&files, Decision::Mount { checkpoint: 1, replay: 7 })));
        assert_eq!(outcome, Outcome::Mount { checkpoint: 1, replay: 7, exhausted: None, store_degraded: false },);
        assert_eq!(outcome.class(), MountClass::Mounted);
        assert!(outcome.admits_mutation());
    }

    /// "a valid checkpoint: mount it, **even if a stale INIT record remains**, then replay".
    #[test]
    fn a_stale_init_witness_does_not_stop_a_valid_checkpoint_mounting() {
        let files = [entry("INIT.REC", SLOT_FILE_LEN as u32), entry("CAT0.CHK", CHECKPOINT_FILE_LEN as u32)];
        let mut observed = shape(&files, Decision::Mount { checkpoint: 0, replay: 0 });
        observed.witness = Some(STORE);
        assert_eq!(classify(None, Some(observed)).class(), MountClass::Mounted);
    }

    /// §5.2's exhausted monotonic space: intact, readable, and refusing mutation until a reset.
    #[test]
    fn an_exhausted_space_mounts_read_only_without_being_a_fault() {
        let files = [entry("CAT0.CHK", CHECKPOINT_FILE_LEN as u32)];
        let decision = Decision::MountReadOnly { checkpoint: 0, replay: 2, reason: ReadOnly::SequenceSpaceExhausted };
        let outcome = classify(None, Some(shape(&files, decision)));
        assert_eq!(outcome.class(), MountClass::Mounted, "an exhausted space is not a degraded store");
        assert!(!outcome.admits_mutation());
    }

    /// §12's class 6 is durable and comes from the header bit, not from anything discovered at a pin.
    #[test]
    fn the_recovery_degraded_bit_is_class_six_and_refuses_mutation() {
        let files = [entry("CAT0.CHK", CHECKPOINT_FILE_LEN as u32)];
        let mut observed = shape(&files, Decision::Mount { checkpoint: 0, replay: 0 });
        observed.store_degraded = true;
        let outcome = classify(None, Some(observed));
        assert_eq!(outcome.class(), MountClass::MountedStoreDegraded);
        assert!(!outcome.admits_mutation(), "class 6 refuses mutations exactly as class 5 does");
    }

    /// §6.3's fail-closed rules arrive here as class 5, with the evidence preserved.
    #[test]
    fn a_journal_fault_mounts_recovery_failed() {
        let files = [entry("CAT0.CHK", CHECKPOINT_FILE_LEN as u32)];
        for fault in [
            FailClosed::AmbiguousCheckpoint,
            FailClosed::NewerEpochRecord { slot: 3 },
            FailClosed::RecordBeyondStop { slot: 9 },
        ] {
            let outcome = classify(None, Some(shape(&files, Decision::Fail(fault))));
            assert_eq!(outcome, Outcome::RecoveryFailed(Fault::Journal(fault)));
            assert!(!outcome.admits_mutation());
        }
    }

    /// §1.1: a lost single-copy FAT structure is a store fault, and it is decided before the
    /// checkpoint observations are trusted — they were read through the structure that is gone.
    #[test]
    fn a_lost_fat_structure_fails_closed_before_the_checkpoint_decision() {
        let files = [entry("CAT0.CHK", CHECKPOINT_FILE_LEN as u32)];
        let mut observed = shape(&files, Decision::Mount { checkpoint: 0, replay: 5 });
        observed.fat_intact = false;
        assert_eq!(classify(None, Some(observed)), Outcome::RecoveryFailed(Fault::LostFatStructure));
    }

    /// The three pre-birth cases, and the fresh card that is not one of them.
    #[test]
    fn the_pre_birth_cases_are_distinguished() {
        // No `/OBC2` at all.
        assert_eq!(classify(None, None), Outcome::Initialize);

        // A `/OBC2` holding nothing but `IMPORT` and staged files. §12: that card "is a fresh card,
        // is initialized, and then imports them" — the caller excludes IMPORT from the listing, so
        // an empty listing is exactly it.
        assert_eq!(classify(None, Some(shape(&[], Decision::NoCheckpoint))), Outcome::Initialize);

        // A valid witness with only its own ordered prefix: resume with that unadvertised StoreId.
        let files = [entry("INIT.REC", SLOT_FILE_LEN as u32), entry("COMMIT.JNL", JOURNAL_FILE_LEN as u32)];
        let mut observed = shape(&files, Decision::NoCheckpoint);
        observed.witness = Some(STORE);
        assert_eq!(classify(None, Some(observed)), Outcome::ResumeInitialization { store: STORE });

        // The same prefix with no witness: remove it and restart with a new StoreId.
        let observed = shape(&files, Decision::NoCheckpoint);
        assert_eq!(classify(None, Some(observed)), Outcome::RestartPreBirth { files: 2 });
        assert_eq!(classify(None, Some(observed)).class(), MountClass::Initializing);
    }

    /// §12: "An unknown name, oversize entry, or valid gate is not a pre-birth prefix and fails
    /// closed."
    #[test]
    fn an_unknown_shape_fails_closed_rather_than_being_reinitialized() {
        // A foreign name.
        let files = [entry("INIT.REC", SLOT_FILE_LEN as u32), entry("STRANGE.DAT", 10)];
        assert_eq!(
            classify(None, Some(shape(&files, Decision::NoCheckpoint))),
            Outcome::RecoveryFailed(Fault::UnknownShape),
        );

        // Out of creation order.
        let files = [entry("COMMIT.JNL", JOURNAL_FILE_LEN as u32), entry("INIT.REC", SLOT_FILE_LEN as u32)];
        assert_eq!(
            classify(None, Some(shape(&files, Decision::NoCheckpoint))),
            Outcome::RecoveryFailed(Fault::UnknownShape),
        );

        // Oversize.
        let files = [entry("INIT.REC", SLOT_FILE_LEN as u32 + 1)];
        assert_eq!(
            classify(None, Some(shape(&files, Decision::NoCheckpoint))),
            Outcome::RecoveryFailed(Fault::UnknownShape),
        );

        // A valid gate on an otherwise perfect prefix: something was born here and its checkpoint is
        // gone, which is corruption rather than a restart.
        let files = [entry("INIT.REC", SLOT_FILE_LEN as u32)];
        let mut observed = shape(&files, Decision::NoCheckpoint);
        observed.any_valid_gate = true;
        assert_eq!(classify(None, Some(observed)), Outcome::RecoveryFailed(Fault::UnknownShape));
    }

    /// §12: "The final entry of that prefix may be short or incomplete: a cut during the zero-fill
    /// of `INIT.REC` or of any preallocated file leaves a truncated last file, which is a bounded
    /// restart case and not a foreign name."
    #[test]
    fn a_truncated_final_file_is_a_restart_case_and_not_a_foreign_name() {
        for short in [0u32, 1, 512, JOURNAL_FILE_LEN as u32 - 1] {
            let files = [entry("INIT.REC", SLOT_FILE_LEN as u32), entry("COMMIT.JNL", short)];
            assert_eq!(prefix_verdict(&files), Prefix::Exact(2), "a {short}-byte COMMIT.JNL");
        }
    }

    /// The amended creation order: seven files and four directories, with no shard tree in either.
    #[test]
    fn the_creation_order_has_no_shard_directories() {
        assert_eq!(CREATION_ORDER.len(), 7);
        assert_eq!(CREATION_ORDER[0].name, "INIT.REC", "the witness is first, before anything can outlive a cut");
        assert_eq!(CREATION_ORDER.last().unwrap().name, "CAT1.CHK");
        assert_eq!(CREATION_DIRECTORIES, ["OBC2", "GEN", "WORK", "IMPORT"]);

        // The zero-fill is unchanged; what went away is 512 make_dir calls and 16 MiB of clusters.
        let total: u64 = CREATION_ORDER.iter().map(|file| u64::from(file.len)).sum();
        assert_eq!(total as usize, INITIALIZATION_BYTES);
        assert_eq!(INITIALIZATION_BYTES, 4_636_672);

        // And an exact full prefix is the state just before the first checkpoint is written.
        let files: Vec<Entry<'_>> = CREATION_ORDER.iter().map(|file| entry(file.name, file.len)).collect();
        assert_eq!(prefix_verdict(&files), Prefix::Exact(7));
    }

    /// The lazy-shard obligation: one `make_dir` per shard, derived from the generation, and the
    /// same shard for both roles.
    #[test]
    fn a_shard_is_named_by_the_generation_that_first_needs_it() {
        let generation = GenerationId::new(0x1234_5678_9ABC_DEF0);
        let (role, shard) = shard_to_create(generation, Role::Gen);
        assert_eq!(role, Role::Gen);
        assert_eq!(shard.index(), 0xF0);
        assert_eq!(shard_to_create(generation, Role::Work).1, shard, "both roles shard on the same byte");
    }

    /// §12 makes class 4 dynamic, one-way, and disjoint from class 6.
    #[test]
    fn a_failed_lazy_pin_moves_a_mounted_store_to_class_four_and_never_back() {
        let outcome = Outcome::Mount { checkpoint: 0, replay: 0, exhausted: None, store_degraded: false };
        let mut state = MountState::new(&outcome);
        assert_eq!(state.class(), MountClass::Mounted);

        state.note_failed_pin();
        assert_eq!(state.class(), MountClass::MountedDegradedEntries);
        assert_eq!(state.degraded_entries(), 1);
        state.note_failed_pin();
        assert_eq!(state.class(), MountClass::MountedDegradedEntries, "the class does not return to 3");
        assert_eq!(state.degraded_entries(), 2);

        // "a recorded store-wide condition is never reported as class `4`".
        let degraded = Outcome::Mount { checkpoint: 0, replay: 0, exhausted: None, store_degraded: true };
        let mut state = MountState::new(&degraded);
        state.note_failed_pin();
        assert_eq!(state.class(), MountClass::MountedStoreDegraded);

        // And a failed pin cannot rescue a recovery-failed mount into a writable one.
        let mut state = MountState::new(&Outcome::RecoveryFailed(Fault::UnknownShape));
        state.note_failed_pin();
        assert_eq!(state.class(), MountClass::RecoveryFailed);
    }

    /// §12.1's table: a name must match a prefix **and** that prefix's extension.
    #[test]
    fn a_staged_name_selects_a_kind_only_on_both_halves() {
        let kind = |text: &str| classify_staged(&ShortName::parse(text).expect(text));
        assert_eq!(kind("UPDATE.BIN"), Some(StagedKind::UpdatePackage));
        assert_eq!(kind("ROUTE001.OBR"), Some(StagedKind::Route));
        assert_eq!(kind("ROUTE.OBR"), Some(StagedKind::Route));
        assert_eq!(kind("MAPALPS.OBM"), Some(StagedKind::VolumeManifest));

        // One half alone selects nothing.
        assert_eq!(kind("ROUTE001.OBM"), None, "the route prefix with the map extension");
        assert_eq!(kind("TRACK001.OBR"), None, "the route extension with no route prefix");
        assert_eq!(kind("UPDATE.OBR"), None);
        assert_eq!(kind("README.TXT"), None);
        // And the three unimportable kinds are unimportable however they are named.
        assert_eq!(kind("WEATHER.OBW"), None);
        assert_eq!(kind("RIDE0001.OBG"), None);
        assert_eq!(kind("TRIP0001.OBT"), None);
    }

    /// The short name is the 11 space-padded bytes §12.1 derives an identity over, and the stripped
    /// stem is what a synthesized map manifest takes its display name from.
    #[test]
    fn a_short_name_is_eleven_space_padded_bytes() {
        let name = ShortName::parse("MAPALPS.OBM").unwrap();
        assert_eq!(&name.0, b"MAPALPS OBM");
        assert_eq!(name.stem(), b"MAPALPS ");
        assert_eq!(name.extension(), b"OBM");
        assert_eq!(name.stripped_stem(), b"MAPALPS");

        assert_eq!(ShortName::parse("UPDATE.BIN").unwrap().0, *b"UPDATE  BIN");
        assert!(ShortName::parse("TOOLONGNAME.OBR").is_none());
        assert!(ShortName::parse("NAME.LONG").is_none());
        assert!(ShortName::parse(".OBR").is_none());
    }

    /// §12.1's per-mount bound, in FAT physical order, with the remainder left for next time.
    #[test]
    fn at_most_eight_staged_files_are_imported_per_mount_in_physical_order() {
        let mut names: Vec<ShortName> = Vec::new();
        for index in 0..12 {
            names.push(ShortName::parse(&std::format!("ROUTE{index:03}.OBR")).unwrap());
        }
        names.push(ShortName::parse("README.TXT").unwrap());
        names.push(ShortName::parse("UPDATE.BIN").unwrap());

        let plan = plan_imports(names.clone());
        assert_eq!(plan.imports.len(), MAX_IMPORTS_PER_MOUNT);
        assert_eq!(plan.ignored, 1, "the unknown name is ignored, never opened or deleted");
        assert_eq!(plan.deferred, 5, "four routes and the update package wait for the next mount");
        // Physical order, not sorted or normalized.
        for (index, (name, kind)) in plan.imports.iter().enumerate() {
            assert_eq!(*name, names[index]);
            assert_eq!(*kind, StagedKind::Route);
        }
    }

    /// A directory a human writes into: an empty one, and one holding nothing importable.
    #[test]
    fn an_import_directory_with_nothing_importable_plans_nothing() {
        assert_eq!(plan_imports([]), ImportPlan { imports: heapless::Vec::new(), ignored: 0, deferred: 0 });
        let plan = plan_imports([ShortName::parse("NOTES.TXT").unwrap(), ShortName::parse("DCIM").unwrap()]);
        assert!(plan.imports.is_empty());
        assert_eq!(plan.ignored, 2);
        assert_eq!(plan.deferred, 0);
    }

    /// The kind values are the object registry's, which §12.1's table states and the import path
    /// publishes under.
    #[test]
    fn the_staged_kinds_are_the_registry_values() {
        assert_eq!(StagedKind::Route as u16, 1);
        assert_eq!(StagedKind::VolumeManifest as u16, 6);
        assert_eq!(StagedKind::UpdatePackage as u16, 7);
    }
}
