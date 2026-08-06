//! Receiving a **volume set** over the link (issue #1039, epic #1016 P3b).
//!
//! A single map is one file and one transfer, so the device needed no state between them. A set is
//! `1..=32` shard files — optionally one of them a **terrain raster** rather than an OBCM shard
//! (#1044) — plus a manifest ([`OBCA_Spec.md` §5](../../../specs/OBCA_Spec.md)), and the
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
//! - Every announce re-states the set's **shard count**, so a host that starts sending a
//!   differently-shaped set mid-transfer is a named mismatch rather than a chimera assembled out of
//!   both. Two sets of the *same* count are not distinguishable here — the descriptor carries
//!   nothing else that identifies a set — and that limit is stated in [`shard_announce`] rather
//!   than papered over: what catches it is the manifest commit's cross-check against the shards
//!   actually on the card, which is later but is still before anything mounts.
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

/// How many **records** the manifest of this session must carry: the OBCM shards plus the terrain
/// shard, if one committed. `OBCA_Spec.md` §5.2's `Shard Count` counts every record, terrain
/// included, so this is the number the announce-time length check is derived from.
const fn records(shard_count: u8, terrain: bool) -> usize {
    shard_count as usize + terrain as usize
}

/// The set upload in flight, or the absence of one. One of these is held by the board's object
/// store for the life of a link; `link_reset` drops it.
///
/// Small, and deliberately no per-shard sizes: what the manifest records about a shard is
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
    /// Whether this set's **terrain shard** has committed (#1044). Not part of `committed`: a
    /// raster is not an OBCM shard, it holds no index in that bitmap, and every rule that counts
    /// shards — the completeness test, the device ceiling — must keep counting only the OBCM files.
    /// What it *does* change is the manifest's length, because §5.2's `Shard Count` counts records.
    terrain: bool,
}

impl SetUpload {
    /// Open a session for a set of `shard_count` shards under the minted set `id`.
    pub const fn new(id: u16, shard_count: u8) -> SetUpload {
        SetUpload { id, shard_count, committed: 0, terrain: false }
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

    /// Record this set's terrain shard as committed. Idempotent, for the same reason [`mark`] is:
    /// a re-sent raster that *succeeds* overwrites the one file and changes nothing about the set's
    /// shape. A re-send that **fails** is not idempotent and must be undone with
    /// [`clear_terrain`](Self::clear_terrain) — the file is deleted, so the session's memory of it
    /// would otherwise outlive the raster.
    ///
    /// [`mark`]: Self::mark
    pub fn mark_terrain(&mut self) {
        self.terrain = true;
    }

    /// Forget this set's terrain shard — the card no longer holds one.
    ///
    /// Called when a raster's transfer is discarded, which deletes `MS{id}.OBD` whether or not an
    /// earlier attempt had committed. Without this, a failed **re-send** of an already-committed
    /// raster left the session claiming `N + 1` records with only `N` files on the card: the
    /// manifest then *passed* the announce-length check and was refused at its commit instead,
    /// taking the whole set with it. Clearing the mark moves that refusal back to the announce,
    /// where it costs the host one descriptor instead of a manifest transfer and a deleted set —
    /// and, better, tells a host that simply re-sends the raster that it may still do so.
    ///
    /// The OBCM shard bitmap has the same shape of gap and it is deliberately **not** fixed the
    /// same way: `set_shard_discard` deletes one shard of a set whose session must survive so the
    /// host can re-send it, and clearing its bit is a separate change with its own blast radius.
    /// Terrain is one optional file with one bit, so it is cheap to keep honest here.
    pub fn clear_terrain(&mut self) {
        self.terrain = false;
    }

    /// Whether this set's terrain shard has committed — the fact that decides whether the manifest
    /// this session will accept is `72 + 56 × N` or `72 + 56 × (N + 1)`.
    pub const fn has_terrain(&self) -> bool {
        self.terrain
    }

    /// Every shard the set names has committed — the precondition §5.4 puts on the manifest.
    ///
    /// Terrain is deliberately **not** part of this. The device cannot know whether the host's
    /// manifest will carry a `terrain` record until the raster arrives (or does not), so
    /// "complete" can only ever mean *every OBCM shard the descriptor promised*. What catches a
    /// host that sent a terrain-bearing manifest without the raster — or a raster the manifest does
    /// not name — is the length check below, which is exact in both directions.
    pub const fn is_complete(&self) -> bool {
        let all = if self.shard_count >= 32 { u32::MAX } else { (1u32 << self.shard_count) - 1 };
        self.committed == all
    }

    /// The manifest length this session's **record** count implies (§5.2), for the announce check:
    /// `72 + 56 × (shards + terrain)`.
    ///
    /// This is the whole of #1044's bug, stated as one expression. The host builds its manifest
    /// over every record — `OBCA_Spec.md` §5.2 is explicit that `Shard Count` counts the terrain
    /// one too — so a set with a raster announces 56 bytes more than a device counting only OBCM
    /// shards expects, and the manifest is refused at the very last transfer of a multi-gigabyte
    /// upload. A set with no terrain is unchanged: `terrain` is false and this is the old formula.
    pub const fn manifest_len(&self) -> u32 {
        obcs::manifest_len(records(self.shard_count, self.terrain)) as u32
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
        // A differently-shaped set, mid-set. Answering `Mismatch` rather than silently re-opening
        // is what keeps a set from being assembled out of two — the shards already on the card
        // belong to the id this session minted, and abandoning them here would strand gigabytes
        // unnamed.
        //
        // **What this cannot see** is a switch between two sets with the *same* shard count: the
        // descriptor carries a count and an index, and neither identifies a set. Such a mix is
        // caught at the manifest's commit instead, where every shard is re-checked against the
        // manifest's own record of it (length, OBCM version, header bbox) — later than here, but
        // still before the set is a map, and a host is required by §5.3 to have proven its set
        // before it offered a byte of it. Closing the gap at the announce would need a set
        // identifier on the wire, i.e. a descriptor change; see `obc-ble-interface-spec.md` §4.1.
        Some(_) => Err(SetReject::Mismatch),
        None => Ok(true),
    }
}

/// Whether the set's **terrain shard** announce may proceed (#1044).
///
/// Two refusals, both before a byte streams:
///
/// - no set is in flight → [`SetReject::Mismatch`]. A raster on its own is not a map and names no
///   set: the set id it would be written under is minted by the *first shard*, so a terrain shard
///   arriving first has no `MS{id}` to belong to. The host's own order — shards, then terrain, then
///   the manifest — is the order §5.4 already asks for, and this is the receiver's half of it.
/// - the set already fills the record space → [`SetReject::Shards`]. §5.2 caps a manifest at 32
///   **records**, so a 32-shard set has no room for a terrain record and the manifest it would need
///   could not be written at all. Refusing here costs the raster's transfer; refusing at the
///   manifest would cost the whole set.
///
/// A **re-sent** terrain shard is allowed, exactly as a re-sent OBCM shard is: it is one
/// independent file, and overwriting it is the cheapest recovery from a single bad transfer.
pub fn terrain_announce(open: Option<&SetUpload>) -> Result<(), SetReject> {
    let Some(session) = open else { return Err(SetReject::Mismatch) };
    if records(session.shard_count, true) > obcs::MAX_SHARDS {
        return Err(SetReject::Shards);
    }
    Ok(())
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
/// - the announced length is not `72 + 56 × Shard Count` → [`SetReject::Length`], where
///   `Shard Count` is every **record** the session saw: the OBCM shards plus the terrain shard, if
///   one committed (#1044). §5.3 rejects a manifest of any other length at parse anyway; catching
///   it here means the rider is not told after the transfer what could be said before it.
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

/// Whether a **just-uploaded** manifest's `terrain` record agrees with the raster that landed
/// beside it (#1044) — the commit-time half of the cross-check, as a pure rule so it can be read
/// and tested without a card.
///
/// `recorded` is the `Bytes` of the manifest's `Role == 3` record, or `None` when the manifest
/// names no raster. `on_card` is the length of `MS{id}.OBD` **if** it opened and its header parses
/// as an OBCT container this firmware reads, and `None` for every way that can fail at once —
/// absent, unopenable, too short, or not an OBCT.
///
/// ## Why this is strict here and deliberately *not* strict at the boot scan
///
/// [`OBCA_Spec.md` §5.3](../../../specs/OBCA_Spec.md) is explicit, and it is the one exception in
/// the whole validation list: **a missing or unreadable terrain shard does not fail the mount.** A
/// reader MUST mount such a set, MUST fall back to no elevation, and MUST NOT present it as a
/// fault, because elevation is an enhancement and a set whose raster will not open is exactly the
/// map it would have been if it had been baked without one.
///
/// That clemency is about a *card*, over time: a rider who deleted `MS7.OBD` to reclaim space, a
/// hand-copied set whose raster was truncated, a transient read glitch, a future OBCT version this
/// firmware will not read. None of those make the map a lie.
///
/// A **commit** is the opposite situation. The host is on the other end of a cable, it built the
/// manifest and the raster together seconds ago, and it has already been told the exact length its
/// manifest must have. A disagreement here is not a card that aged — it is the two ends
/// disagreeing about what was just transferred, which is precisely the class of bug #1044 was. So
/// the manifest is refused and the set deleted whole, and the rider re-sends rather than mounting
/// a map whose manifest does not describe its own files.
///
/// The asymmetry is the point, and it is stated in both directions so neither call site can be
/// "simplified" into the other.
pub const fn terrain_record_agrees(recorded: Option<u32>, on_card: Option<u32>) -> bool {
    match (recorded, on_card) {
        // The manifest names a raster and one of exactly that size is beside it. Its bbox needs no
        // check here: §5.3 pins it to the assembly bbox at *parse*, and an OBCT header carries no
        // bbox to compare against anyway.
        (Some(bytes), Some(len)) => bytes == len,
        // The manifest names a raster and there is none. The upload said otherwise.
        (Some(_), None) => false,
        // No terrain record. Whatever `MS{id}.OBD` may be beside it is not this set's business:
        // `set_upload_begin` clears every derived name of the id before the first byte streams, so
        // an upload cannot leave one, and a leftover from a hand-made card is debris the next
        // upload's supersede pass reclaims — not grounds to refuse a manifest that never claimed it.
        (None, _) => true,
    }
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

/// What a card file's leading four bytes turned out to be — the sweep's only evidence.
///
/// The three cases are **not** collapsible into `Option<[u8; 4]>`, which is what the first cut of
/// this did and what left a whole class of torn upload on the card forever: "the file holds fewer
/// than four bytes" is not "the file could not be read". The first is a state only *this device*
/// produces (a create-then-write that did not reach its write), and the second is a bus glitch,
/// which must never green-light a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootMagic {
    /// The first four bytes, as read.
    Bytes([u8; 4]),
    /// The file opened, and holds fewer than four bytes.
    Short,
    /// The file could not be opened or read at all.
    Unreadable,
}

/// Whether `magic` is a magic **this device was in the middle of writing**: all zeros (the
/// placeholder), or a strict prefix of `full` zero-padded (a magic patch that tore part-way — the
/// four bytes are one `write`, but a `write` is not one sector and a power cut splits it).
///
/// Nothing else produces those shapes. A file that arrived any other way — a card reader, another
/// device — carries its whole magic from the first block that reaches the card, because the host
/// copying it holds the finished file and writes it front to back.
const fn is_torn_magic(magic: [u8; 4], full: [u8; 4]) -> bool {
    let mut prefix = 0;
    while prefix < 4 {
        let mut i = 0;
        let mut matches = true;
        while i < 4 {
            let expect = if i < prefix { full[i] } else { 0 };
            if magic[i] != expect {
                matches = false;
            }
            i += 1;
        }
        if matches {
            return true;
        }
        prefix += 1;
    }
    false
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
/// `magic` is what reading the manifest file's first four bytes produced ([`RootMagic`]). An
/// **unreadable** file is never claimed, exactly as the map sweep refuses to claim one.
///
/// Three shapes are the sweep's, and the two beyond the plain placeholder are the ones a first cut
/// left on the card forever:
///
/// - **Four zero bytes** — the token, written and never patched.
/// - **Shorter than four bytes** ([`RootMagic::Short`]) — the token's own `create` landed and its
///   4-byte write did not, so the card holds a size-0 (or size-1..3) `.OBS`. That is *more*
///   certainly ours than four zeros: a manifest is at least 72 bytes by construction (§5.2), so no
///   complete one is ever this short, and no reader will ever accept it. Keeping it was the worse
///   half of the trade — the name is what shields its shards from the orphan pass, so a zero-byte
///   file froze gigabytes with nothing left to name them.
/// - **A torn magic patch** — `OBC\0` and friends: the commit's four-byte write split by a power
///   cut. The set is invisible either way, so leaving it costs the same gigabytes.
///
/// The residual, stated rather than buried: a rider whose *own* card-reader copy was interrupted
/// inside the manifest's first four bytes loses that set to the sweep. It is a set that could not
/// mount and would have to be re-copied whole regardless, and the shape is far rarer than the power
/// cut this reclaims — a copy writes the manifest's magic in its first block, so the window is one
/// block wide against a whole transfer's.
pub fn sweep_verdict(magic: RootMagic) -> SweepVerdict {
    match magic {
        RootMagic::Short => SweepVerdict::Reclaim,
        RootMagic::Bytes(bytes) if is_torn_magic(bytes, obcs::MAGIC) => SweepVerdict::Reclaim,
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
/// So an orphan is reclaimed **only** on a magic this device was mid-write on — zero, or a torn
/// `OBC\0`-shaped patch — and a complete-looking orphan is left for the manifest that is probably
/// still coming. The cost of being wrong in this direction is some dead bytes a later upload's
/// supersede pass reclaims (§5.4's writer SHOULD, which the device honours by deleting the set it
/// replaces); the cost of being wrong in the other direction is the rider's map.
///
/// **A shard shorter than four bytes is kept**, and that is the one place this deliberately differs
/// from [`sweep_verdict`]. The two rules weigh the same risk against different stakes: a manifest's
/// mere *name* shields a whole set's shards from this pass, so a truncated one strands gigabytes and
/// is worth reclaiming; a truncated shard **is** the three bytes it holds, so reclaiming it buys
/// nothing measurable and can only be wrong — a card-reader copy that has just begun looks exactly
/// like this.
pub fn orphan_shard_verdict(magic: RootMagic) -> SweepVerdict {
    match magic {
        RootMagic::Bytes(bytes) if is_torn_magic(bytes, obc_formats::obcm::MAGIC) => SweepVerdict::Reclaim,
        _ => SweepVerdict::Keep,
    }
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

    /// **The #1044 regression, pinned.** A set that carries a terrain shard has one more *record*
    /// than it has OBCM shards, so its manifest is 56 bytes longer — and a device that counted only
    /// shards refused it with `Length` at the very last transfer of a multi-gigabyte upload.
    #[test]
    fn a_terrain_shard_lengthens_the_manifest_by_one_record() {
        let mut session = SetUpload::new(5, 3);
        for index in 0..3 {
            session.mark(index);
        }
        let without = session.manifest_len();
        assert_eq!(without, obcs::manifest_len(3) as u32, "three shards, three records");
        assert!(!session.has_terrain());

        assert_eq!(terrain_announce(Some(&session)), Ok(()));
        session.mark_terrain();
        assert!(session.has_terrain());
        assert!(session.is_complete(), "a raster is not a shard — completeness is unchanged");
        assert_eq!(session.shard_count(), 3, "…and neither is the shard count");

        let with = session.manifest_len();
        assert_eq!(with, without + obcs::SHARD_RECORD_LEN as u32, "exactly one more 56-byte record");
        assert_eq!(with, obcs::manifest_len(4) as u32);
        assert_eq!(manifest_announce(Some(&session), with), Ok(()));
        assert_eq!(
            manifest_announce(Some(&session), without),
            Err(SetReject::Length),
            "the terrain-less length is now the wrong one"
        );
        // Idempotent: a re-sent raster that lands is one file overwritten, not a second record.
        session.mark_terrain();
        assert_eq!(session.manifest_len(), with);
    }

    /// A re-send that **fails** deletes `MS{id}.OBD`, so the session must stop counting it — and
    /// the manifest that follows must be refused at the *announce*, not at the commit that would
    /// delete the whole set.
    #[test]
    fn a_discarded_raster_stops_counting_toward_the_manifest() {
        let mut session = SetUpload::new(8, 2);
        session.mark(0);
        session.mark(1);
        session.mark_terrain();
        let with = session.manifest_len();

        session.clear_terrain();
        assert!(!session.has_terrain());
        assert_eq!(session.manifest_len(), with - obcs::SHARD_RECORD_LEN as u32);
        assert_eq!(
            manifest_announce(Some(&session), with),
            Err(SetReject::Length),
            "the host is told before it sends a manifest naming a raster the card no longer has"
        );
        // …and the set is still whole: re-sending the raster puts it back.
        assert!(session.is_complete());
        assert_eq!(terrain_announce(Some(&session)), Ok(()));
        session.mark_terrain();
        assert_eq!(manifest_announce(Some(&session), with), Ok(()));
    }

    /// A set with **no** terrain is byte-for-byte the set it was before #1044 — the property that
    /// makes the wire change additive rather than a break.
    #[test]
    fn a_set_without_terrain_is_unchanged() {
        let mut session = SetUpload::new(6, 2);
        session.mark(0);
        session.mark(1);
        assert_eq!(session.manifest_len(), (obcs::HEADER_LEN + 2 * obcs::SHARD_RECORD_LEN) as u32);
        assert_eq!(manifest_announce(Some(&session), session.manifest_len()), Ok(()));
        assert_eq!(
            manifest_announce(Some(&session), session.manifest_len() + obcs::SHARD_RECORD_LEN as u32),
            Err(SetReject::Length),
            "a manifest that names a raster this set never sent is refused too"
        );
    }

    /// A terrain shard names no set of its own: the id is minted by the first OBCM shard, so a
    /// raster with nothing in flight has no `MS{id}` to be written under.
    #[test]
    fn a_terrain_shard_with_no_set_in_flight_is_refused() {
        assert_eq!(terrain_announce(None), Err(SetReject::Mismatch));
    }

    /// §5.2 caps a manifest at 32 **records**, so a 32-shard set has no room for a terrain record —
    /// and the honest moment to say so is the raster's announce, not the manifest's.
    #[test]
    fn a_full_set_has_no_room_for_a_terrain_record() {
        let full = SetUpload::new(1, 32);
        assert_eq!(terrain_announce(Some(&full)), Err(SetReject::Shards));
        let one_short = SetUpload::new(1, 31);
        assert_eq!(terrain_announce(Some(&one_short)), Ok(()), "31 shards + terrain is exactly the cap");
    }

    /// The commit-time terrain cross-check, and the boundary of what it is allowed to judge.
    #[test]
    fn a_committed_manifests_terrain_record_must_match_the_raster_beside_it() {
        assert!(terrain_record_agrees(Some(6_192), Some(6_192)), "the record and the file agree");
        assert!(!terrain_record_agrees(Some(6_192), Some(6_191)), "one byte out is still a lie");
        assert!(!terrain_record_agrees(Some(6_192), None), "a manifest that names a raster there is none of");
        // A manifest with no terrain record judges nothing about a file it never claimed. The
        // upload cannot have left one (`set_upload_begin` clears the id first) and a hand-made
        // card's leftover is debris, not grounds to refuse a set.
        assert!(terrain_record_agrees(None, None));
        assert!(terrain_record_agrees(None, Some(4_096)));
    }

    /// **The asymmetry, pinned.** §5.3 makes a missing or unreadable raster a *mount-time*
    /// non-failure — the set MUST still mount, flat. This predicate is the **commit** rule and is
    /// deliberately harsher; asserting that difference here is what stops a later reader from
    /// "unifying" the two and taking a rider's whole map away because they deleted an `.OBD`.
    #[test]
    fn the_commit_rule_is_stricter_than_the_mount_rule_on_purpose() {
        // At commit: refused. At mount: §5.3 says the set is a map with flat profiles, which is
        // why `Storage::set_file_totals` never consults this and only the commit path does.
        assert!(!terrain_record_agrees(Some(1), None));
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

    /// The sweep claims the signatures only this device can produce, and refuses everything else.
    #[test]
    fn only_a_zero_magic_manifest_is_the_sweeps_to_reclaim() {
        assert_eq!(sweep_verdict(RootMagic::Bytes([0, 0, 0, 0])), SweepVerdict::Reclaim);
        assert_eq!(sweep_verdict(RootMagic::Bytes(obcs::MAGIC)), SweepVerdict::Keep, "a real manifest is a map");
        assert_eq!(sweep_verdict(RootMagic::Bytes(*b"OBCM")), SweepVerdict::Keep, "so is anything else intact");
        assert_eq!(sweep_verdict(RootMagic::Unreadable), SweepVerdict::Keep, "an unreadable file is never claimed");
    }

    /// **A manifest too short to hold a magic is the sweep's** — the case that used to be
    /// indistinguishable from "unreadable" and therefore kept forever. The window is real: the
    /// token's directory entry is committed by the `create`, and the four bytes are a second write.
    /// A `.OBS` of 0–3 bytes cannot be a manifest (§5.2 puts the floor at 72), so nothing is being
    /// guessed at.
    #[test]
    fn a_manifest_too_short_to_hold_a_magic_is_reclaimed() {
        assert_eq!(sweep_verdict(RootMagic::Short), SweepVerdict::Reclaim);
        assert_ne!(
            sweep_verdict(RootMagic::Short),
            sweep_verdict(RootMagic::Unreadable),
            "short and unreadable are different facts and must not share a verdict"
        );
    }

    /// **A magic patch that tore is the sweep's too.** The commit is one four-byte write, and one
    /// write is not one sector: a power cut inside it leaves a strict prefix of `OBCS`. The set is
    /// invisible to every reader either way, so keeping it froze the gigabytes beside it.
    #[test]
    fn a_half_patched_manifest_magic_is_reclaimed() {
        for magic in [*b"O\0\0\0", *b"OB\0\0", *b"OBC\0"] {
            assert_eq!(sweep_verdict(RootMagic::Bytes(magic)), SweepVerdict::Reclaim, "{magic:?}");
        }
        assert_eq!(sweep_verdict(RootMagic::Bytes(*b"OBCS")), SweepVerdict::Keep, "…and the whole magic is a map");
        // Not a prefix — a different file that happens to start with an O.
        assert_eq!(sweep_verdict(RootMagic::Bytes(*b"OBS\0")), SweepVerdict::Keep);
        assert_eq!(sweep_verdict(RootMagic::Bytes(*b"O\0C\0")), SweepVerdict::Keep, "a prefix is contiguous");
    }

    /// …and the same proof governs an orphan shard against **its** magic, so a set mid-copy over a
    /// card reader survives the boot it is interrupted by.
    #[test]
    fn a_complete_orphan_shard_survives_the_sweep() {
        assert_eq!(orphan_shard_verdict(RootMagic::Bytes(*b"OBCM")), SweepVerdict::Keep, "the rider's mid-copy set");
        assert_eq!(orphan_shard_verdict(RootMagic::Unreadable), SweepVerdict::Keep);
        assert_eq!(orphan_shard_verdict(RootMagic::Bytes([0, 0, 0, 0])), SweepVerdict::Reclaim, "our abandoned stream");
        assert_eq!(orphan_shard_verdict(RootMagic::Bytes(*b"OBC\0")), SweepVerdict::Reclaim, "a torn shard patch");
        // A shard's magic is OBCM, not OBCS: a `.OBS` prefix is not a torn shard, it is a foreign file.
        assert_eq!(orphan_shard_verdict(RootMagic::Bytes(*b"OBCS")), SweepVerdict::Keep);
    }

    /// The one place the two verdicts diverge, asserted so it cannot be "simplified" back into an
    /// alias: a truncated shard is kept, because it *is* its three bytes, while a truncated
    /// manifest is reclaimed, because its name shields a whole set's worth of them.
    #[test]
    fn a_truncated_shard_is_kept_where_a_truncated_manifest_is_not() {
        assert_eq!(orphan_shard_verdict(RootMagic::Short), SweepVerdict::Keep);
        assert_eq!(sweep_verdict(RootMagic::Short), SweepVerdict::Reclaim);
    }

    /// **What the announce cannot see** (`obc-ble-interface-spec.md` §4.1 rule 1). The session is
    /// re-stated by the shard count and nothing else, so a host that switches to a *different* set
    /// with the same count is not detectable here — the mismatch it names is a count mismatch. The
    /// claim is pinned honestly rather than left as prose the code does not back: the switch is
    /// caught at the manifest's commit, by the cross-check against the shards actually on the card.
    #[test]
    fn a_same_count_switch_is_not_visible_at_the_announce() {
        let session = SetUpload::new(4, 6);
        assert_eq!(
            shard_announce(Some(&session), 6, 1, DEVICE_MAX),
            Ok(false),
            "a shard of another six-shard set joins this session — the announce has nothing to tell them apart"
        );
        assert_eq!(
            shard_announce(Some(&session), 7, 1, DEVICE_MAX),
            Err(SetReject::Mismatch),
            "a different count is what the announce *can* see"
        );
    }

    /// A shard arriving **after** the set's manifest committed starts a new set rather than
    /// re-opening the finished one (`obc-ble-interface-spec.md` §4.1). The committed set has a
    /// manifest naming exactly the files it names; appending to it would make that manifest a lie,
    /// so the only safe reading of a shard with no session is "this is a new set".
    #[test]
    fn a_shard_after_a_committed_manifest_opens_a_new_set() {
        let mut session = SetUpload::new(3, 1);
        session.mark(0);
        assert_eq!(manifest_announce(Some(&session), session.manifest_len()), Ok(()));
        // The commit closes the session (`ObjectStore::set_manifest_finish` clears it).
        assert_eq!(shard_announce(None, 1, 0, DEVICE_MAX), Ok(true), "a fresh set, a fresh id");
    }
}
