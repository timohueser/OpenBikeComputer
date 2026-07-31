//! Receiving a **volume set** over the link (issue #1039, epic #1016 P3b).
//!
//! A single map is one file and one transfer, so the device needed no state between them. A set is
//! `1..=32` shard files plus a manifest ([`OBCA_Spec.md` §5](../../../specs/OBCA_Spec.md)), and the
//! one rule that makes a half-uploaded set harmless — **the manifest is written last** (§5.4) — is a
//! rule about the *order of several transfers*. That is state, and this module is it.
//!
//! Like [`crate::map_catalog`], it lives here rather than on the board because the board crate has
//! no `test` harness in CI: the decisions below are the ones a review has to be able to check, and
//! they are pure functions over a struct the board holds one of.
//!
//! ## What is enforced, and why it is enforced rather than trusted
//!
//! §5.4 addresses the *writer*: "a writer MUST transfer every shard first and write the manifest
//! last". A device cannot hold a host to that by reading the spec at it. So the receiving side
//! turns the writer's MUST into a receiver's refusal:
//!
//! - A manifest announced before every shard it will name has committed is answered **before a
//!   byte streams** ([`SetReject::ManifestEarly`]). A host that gets the order wrong learns so in
//!   milliseconds, not after uploading gigabytes into a set that would never mount.
//! - The device holds the shard ceiling, and holds it at the **first** announce, because the
//!   descriptor states the set's shard count in every part (`obc_ble::SetPart`). Refusing a
//!   12-shard set at shard 0 costs the rider nothing; refusing it when the manifest arrives costs
//!   them the whole upload.
//! - Every announce re-states the set it belongs to, so two interleaved sets are a named mismatch
//!   rather than a chimera assembled out of both.
//!
//! ## The token, and what makes the cleanup safe
//!
//! The board opens `MS{id}.OBS` with four zero bytes where the `OBCS` magic goes *before the first
//! shard streams*, and patches the magic in as the very last write of the whole set — the same
//! held-back-magic commit point a single map uses (`obc_ble::HeldMagic`), lifted one level up from
//! a file to a set. That placeholder is what makes an abandoned set **identifiable**: a zero-magic
//! manifest is a set this device was in the middle of writing and nothing else, because a set
//! arriving any other way (a card reader, a second device) either has its whole manifest or has
//! none at all. [`sweep_verdict`] is that rule, and the grace it grants a rider mid-copy.

use obc_formats::obcs;

/// Why a set-upload announce was refused. The board maps these onto the wire's
/// `obc_ble::TransferStatus`; they are named for the *reason* rather than the status so the
/// mapping stays visible at the one place it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetReject {
    /// The descriptor's part field does not name a file of a set — a zero shard count, or an index
    /// at or past it.
    Part,
    /// The set has more shards than this device can hold handles for. Refused at the **first**
    /// announce, because every shard states the count.
    Shards,
    /// The announce contradicts the set already being received: a different shard count, or a
    /// manifest with no set open at all.
    Mismatch,
    /// The manifest was announced before every shard it will name had committed (§5.4).
    ManifestEarly,
    /// The announced length cannot be the thing it claims to be — a manifest that is not
    /// `72 + 56 × Shard Count` bytes.
    Length,
}

/// The set upload in flight, or the absence of one. One of these is held by the board's object
/// store for the life of a link; `link_reset` drops it.
///
/// Eight bytes, and deliberately no per-shard sizes: what the manifest records about a shard is
/// re-checked against the card at commit by reading the shards back (the same pass the boot scan
/// runs), so remembering it here would be a second copy of a fact the card already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetUpload {
    /// The set id this device minted, i.e. the `{id}` of `MS{id}.OBS`.
    id: u16,
    /// How many shards the set has, as every part's descriptor states it.
    shard_count: u8,
    /// One bit per committed shard index. `u32` is exactly the spec's 32-shard cap.
    committed: u32,
}

impl SetUpload {
    /// Open a session for a set of `shard_count` shards under the minted set `id`.
    pub const fn new(id: u16, shard_count: u8) -> SetUpload {
        SetUpload { id, shard_count, committed: 0 }
    }

    pub const fn id(&self) -> u16 {
        self.id
    }

    pub const fn shard_count(&self) -> u8 {
        self.shard_count
    }

    /// Whether shard `index` has committed in this session.
    pub const fn has(&self, index: u8) -> bool {
        index < 32 && (self.committed >> index) & 1 == 1
    }

    /// How many distinct shards have committed.
    pub const fn received(&self) -> u32 {
        self.committed.count_ones()
    }

    /// Record shard `index` as committed. Idempotent — a host re-sending a shard it already
    /// committed (the per-file retry §5.4's independent files make possible) overwrites the file
    /// and leaves this unchanged.
    pub fn mark(&mut self, index: u8) {
        if index < self.shard_count {
            self.committed |= 1 << index;
        }
    }

    /// Every shard the set names has committed — the precondition §5.4 puts on the manifest.
    pub const fn is_complete(&self) -> bool {
        let all = if self.shard_count >= 32 { u32::MAX } else { (1u32 << self.shard_count) - 1 };
        self.committed == all
    }

    /// The manifest length this set's shard count implies (§5.2), for the announce check.
    pub const fn manifest_len(&self) -> u32 {
        obcs::manifest_len(self.shard_count as usize) as u32
    }
}

/// Whether a **shard** announce may proceed against the current session, and what the session
/// should become.
///
/// `open` is the set upload already in flight, if any; `max_shards` is this device's own ceiling
/// (`OBCA_Spec.md` §5.2 allows 32, a device may hold fewer handles). `Ok(true)` means the caller
/// must open a fresh session — it is the first shard of a new set, and the caller mints the id.
///
/// The order of the refusals matters: a malformed part is answered before the ceiling, so a host
/// that packs the field wrong is told *that* rather than "too many shards".
pub fn shard_announce(open: Option<&SetUpload>, shard_count: u8, index: u8, max_shards: u8) -> Result<bool, SetReject> {
    if shard_count == 0 || index >= shard_count || shard_count as usize > obcs::MAX_SHARDS {
        return Err(SetReject::Part);
    }
    if shard_count > max_shards {
        return Err(SetReject::Shards);
    }
    match open {
        // A shard of the set already in flight. A repeat of one that committed is allowed on
        // purpose: shards are independent files (§5.4), so re-sending one is the cheapest possible
        // recovery from a single bad transfer and costs the set nothing.
        Some(session) if session.shard_count == shard_count => Ok(false),
        // A different set, mid-set. Answering `Mismatch` rather than silently re-opening is what
        // keeps a set from being assembled out of two — the shards already on the card belong to
        // the id this session minted, and abandoning them here would strand gigabytes unnamed.
        Some(_) => Err(SetReject::Mismatch),
        None => Ok(true),
    }
}

/// Whether the **manifest** announce may proceed: §5.4's manifest-last rule, as a receiver states
/// it. `total_len` is the announced object size.
///
/// Three refusals, all before any byte streams:
///
/// - no set is in flight at all → [`SetReject::Mismatch`]. A manifest on its own describes files
///   this device never received; writing it would mint a map out of nothing.
/// - a shard the manifest will name has not committed → [`SetReject::ManifestEarly`]. **This is
///   the enforcement.** It is checked against the *session*, not against the card, because the
///   card cannot tell a shard this upload wrote from one that was already there.
/// - the announced length is not `72 + 56 × Shard Count` → [`SetReject::Length`]. §5.3 rejects a
///   manifest of any other length at parse anyway; catching it here means the rider is not told
///   after the transfer what could be said before it.
pub fn manifest_announce(open: Option<&SetUpload>, total_len: u32) -> Result<(), SetReject> {
    let Some(session) = open else { return Err(SetReject::Mismatch) };
    if !session.is_complete() {
        return Err(SetReject::ManifestEarly);
    }
    if total_len != session.manifest_len() {
        return Err(SetReject::Length);
    }
    Ok(())
}

/// What the boot sweep should do with one `MS{id}.OBS` the card root holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepVerdict {
    /// Leave it alone.
    Keep,
    /// A set this device abandoned mid-write: reclaim the whole thing through
    /// `obc_formats::obcs::delete_plan` — manifest first, then every shard name to the cap.
    Reclaim,
}

/// The boot sweep's rule for a volume set, and the reason it needs no age or grace timer.
///
/// The single-map sweep already reclaims exactly one signature — a `MP{id}.OBM` whose held-back
/// magic was never patched in — and refuses to touch anything else, because a file with its magic
/// intact might be the rider's own. A set needs the same guarantee across *several* files, and
/// gets it from the same trick one level up: the board creates `MS{id}.OBS` with four zero bytes
/// before the first shard streams, and patches `OBCS` in only after every shard has landed and the
/// manifest has validated.
///
/// So a **zero-magic manifest is proof**, not a heuristic:
///
/// - This device wrote it. Nothing else on the card creates a `.OBS` at all.
/// - The set is incomplete. The magic goes in last, so a zero-magic manifest means the write never
///   reached its final four bytes.
///
/// And the rider mid-copy is safe **without a grace period**, which is the property worth stating
/// plainly: a set arriving over a card reader is copied file by file from a host that already
/// holds a complete manifest, so at every instant its `MS{id}.OBS` is either absent or whole. It
/// is never zeroed. The sweep therefore cannot see a half-copied set as its own abandoned one, and
/// does not need to wait a boot, an hour, or a heuristic to find out.
///
/// `magic` is the manifest file's first four bytes as read back, or `None` if they could not be
/// read — an unreadable file is never claimed, exactly as the map sweep refuses to claim one.
pub fn sweep_verdict(magic: Option<[u8; 4]>) -> SweepVerdict {
    match magic {
        Some([0, 0, 0, 0]) => SweepVerdict::Reclaim,
        _ => SweepVerdict::Keep,
    }
}

/// Whether an **orphan shard** — an `MS{id}S{kk}.OBM` with no `MS{id}.OBS` beside it at all — is
/// the sweep's to reclaim (`OBCA_Spec.md` §5.4: "shard files with no manifest referencing them are
/// orphans and MAY be deleted").
///
/// The MAY is doing real work here, and this takes the conservative half of it. Two cards produce
/// a manifest-less shard and they must be told apart:
///
/// - **Ours**: a shard stream that died before its own magic was patched in, whose set token was
///   already reclaimed (the manifest delete succeeded, a shard delete did not, then the power
///   went). Zero-magic, so it is claimable by the same proof the map sweep uses.
/// - **The rider's**: a set being copied over a card reader, shards first. Every one of those has
///   its `OBCM` magic from the moment it is complete, and the manifest simply has not been copied
///   yet. Deleting those would destroy a map mid-copy — the single worst thing this sweep could
///   do, and unrecoverable without the whole download again.
///
/// So an orphan is reclaimed **only** on the zero-magic signature, and a complete-looking orphan is
/// left for the manifest that is probably still coming. The cost of being wrong in this direction
/// is some dead bytes a later upload's supersede pass reclaims (§5.4's writer SHOULD, which the
/// device honours by deleting the set it replaces); the cost of being wrong in the other direction
/// is the rider's map.
pub fn orphan_shard_verdict(magic: Option<[u8; 4]>) -> SweepVerdict {
    sweep_verdict(magic)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This board's ceiling in the tests below — `sd::SD_SET_MAX_SHARDS`, the handle budget #1026
    /// bought (16 FAT handles − the 5-handle mid-ride peak).
    const DEVICE_MAX: u8 = 11;

    #[test]
    fn the_first_shard_opens_a_session_and_the_rest_join_it() {
        assert_eq!(shard_announce(None, 3, 0, DEVICE_MAX), Ok(true), "no session yet ⇒ mint one");
        let mut session = SetUpload::new(7, 3);
        assert_eq!(shard_announce(Some(&session), 3, 1, DEVICE_MAX), Ok(false));
        assert_eq!(session.id(), 7);
        assert_eq!(session.shard_count(), 3);
        assert_eq!(session.received(), 0);

        session.mark(0);
        session.mark(1);
        assert!(session.has(0) && session.has(1) && !session.has(2));
        assert_eq!(session.received(), 2);
        // Re-sending a shard is a legal recovery, and does not double-count.
        session.mark(1);
        assert_eq!(session.received(), 2);
        assert_eq!(shard_announce(Some(&session), 3, 1, DEVICE_MAX), Ok(false), "a re-send is allowed");
    }

    /// The headline refusal: a manifest may not arrive until every shard it will name has.
    #[test]
    fn a_manifest_before_its_shards_is_refused() {
        let mut session = SetUpload::new(1, 3);
        let len = session.manifest_len();
        assert_eq!(manifest_announce(Some(&session), len), Err(SetReject::ManifestEarly), "no shards at all");
        session.mark(0);
        session.mark(2);
        assert_eq!(
            manifest_announce(Some(&session), len),
            Err(SetReject::ManifestEarly),
            "a hole in the middle is still an incomplete set"
        );
        session.mark(1);
        assert!(session.is_complete());
        assert_eq!(manifest_announce(Some(&session), len), Ok(()));
    }

    /// …and a manifest with no set in flight at all is refused too, which is the same rule seen
    /// from the other end: there is nothing for it to be the last write of.
    #[test]
    fn a_manifest_with_no_shards_uploaded_is_not_a_map() {
        assert_eq!(manifest_announce(None, 128), Err(SetReject::Mismatch));
    }

    /// The manifest's own length is fixed by the shard count (§5.2), so a wrong one is refused
    /// before the transfer rather than at parse afterwards.
    #[test]
    fn a_manifest_of_the_wrong_length_is_refused_at_the_announce() {
        let mut session = SetUpload::new(2, 2);
        session.mark(0);
        session.mark(1);
        let right = session.manifest_len();
        assert_eq!(right, (obcs::HEADER_LEN + 2 * obcs::SHARD_RECORD_LEN) as u32);
        assert_eq!(manifest_announce(Some(&session), right), Ok(()));
        assert_eq!(manifest_announce(Some(&session), right - 1), Err(SetReject::Length));
        assert_eq!(manifest_announce(Some(&session), right + 56), Err(SetReject::Length));
    }

    /// The ceiling is a *first-announce* refusal, which is the whole reason the shard count rides
    /// every descriptor: a set this device cannot mount costs zero bytes to refuse.
    #[test]
    fn a_set_past_the_device_ceiling_is_refused_at_the_first_shard() {
        assert_eq!(shard_announce(None, DEVICE_MAX + 1, 0, DEVICE_MAX), Err(SetReject::Shards));
        assert_eq!(shard_announce(None, DEVICE_MAX, 0, DEVICE_MAX), Ok(true), "exactly at the cap is fine");
        // The spec's own 32 cap is refused as a malformed part, not as a device limit: no device
        // ceiling can make 33 shards legal.
        assert_eq!(shard_announce(None, 33, 0, 64), Err(SetReject::Part));
    }

    /// A part field that cannot name a file of a set is refused before anything else is considered.
    #[test]
    fn a_part_that_names_no_file_is_refused_first() {
        assert_eq!(shard_announce(None, 0, 0, DEVICE_MAX), Err(SetReject::Part), "a set of zero shards");
        assert_eq!(shard_announce(None, 3, 3, DEVICE_MAX), Err(SetReject::Part), "index == count");
        assert_eq!(shard_announce(None, 3, 200, DEVICE_MAX), Err(SetReject::Part));
        // Malformed beats over-cap: the host is told what is actually wrong with its descriptor.
        assert_eq!(shard_announce(None, 40, 40, DEVICE_MAX), Err(SetReject::Part));
    }

    /// Two sets cannot be interleaved into one.
    #[test]
    fn a_shard_of_a_different_set_is_a_mismatch_not_a_new_session() {
        let session = SetUpload::new(4, 3);
        assert_eq!(shard_announce(Some(&session), 5, 0, DEVICE_MAX), Err(SetReject::Mismatch));
    }

    /// The single-file fast path (§5.5) is a set of one and needs no special case anywhere.
    #[test]
    fn a_set_of_one_is_an_ordinary_set() {
        assert_eq!(shard_announce(None, 1, 0, DEVICE_MAX), Ok(true));
        let mut session = SetUpload::new(9, 1);
        assert!(!session.is_complete());
        session.mark(0);
        assert!(session.is_complete());
        assert_eq!(session.manifest_len(), obcs::manifest_len(1) as u32);
        assert_eq!(manifest_announce(Some(&session), session.manifest_len()), Ok(()));
    }

    /// A 32-shard set exercises the bitmap's top bit — the shift that would overflow if it were
    /// written `(1 << 32) - 1`.
    #[test]
    fn the_completeness_bitmap_holds_the_specs_full_cap() {
        let mut session = SetUpload::new(1, 32);
        for index in 0..32u8 {
            assert!(!session.is_complete(), "incomplete until the last one");
            session.mark(index);
        }
        assert!(session.is_complete());
        assert_eq!(session.received(), 32);
        assert!(!session.has(32), "an index off the end is never held");
    }

    /// The sweep claims exactly one signature, and it is the one only this device can produce.
    #[test]
    fn only_a_zero_magic_manifest_is_the_sweeps_to_reclaim() {
        assert_eq!(sweep_verdict(Some([0, 0, 0, 0])), SweepVerdict::Reclaim);
        assert_eq!(sweep_verdict(Some(obcs::MAGIC)), SweepVerdict::Keep, "a real manifest is a map");
        assert_eq!(sweep_verdict(Some(*b"OBCM")), SweepVerdict::Keep, "so is anything else intact");
        assert_eq!(sweep_verdict(None), SweepVerdict::Keep, "an unreadable file is never claimed");
    }

    /// …and the same proof governs an orphan shard, so a set mid-copy over a card reader survives
    /// the boot it is interrupted by.
    #[test]
    fn a_complete_orphan_shard_survives_the_sweep() {
        assert_eq!(orphan_shard_verdict(Some(*b"OBCM")), SweepVerdict::Keep, "the rider's mid-copy set");
        assert_eq!(orphan_shard_verdict(None), SweepVerdict::Keep);
        assert_eq!(orphan_shard_verdict(Some([0, 0, 0, 0])), SweepVerdict::Reclaim, "our abandoned stream");
    }
}
