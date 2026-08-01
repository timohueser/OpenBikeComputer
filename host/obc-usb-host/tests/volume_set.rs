//! The volume-set transfer, end to end, with no hardware (issue #1039).
//!
//! A set upload is the first thing on this link whose correctness lives *between* transfers:
//! `OBCA_Spec.md` §5.4 says the manifest is written last, and neither half of that sentence can be
//! checked by looking at one descriptor. So this file closes the loop the only way that proves
//! anything — a real assembled set, the real host planner, the real device *decisions*, and a real
//! mount and render off the files that land.
//!
//! ## What is real here and what is a stand-in
//!
//! Real, and the same code the device runs: the OBCS codec and the derived `MS{id}` filenames
//! (`obc_formats::obcs`), the transfer descriptor and `SetPart` packing (`obc_ble`), the whole-object
//! `Receiver` and its CRC, the held-back-magic `HeldMagic`, every announce decision
//! (`obc_app::set_upload`), the sweep verdicts, `obcs::delete_plan`'s ordering, and the reader's
//! mount + the renderer.
//!
//! A stand-in: [`SimCard`]'s FAT calls, which are `std::fs` here and `embedded-sdmmc` on the board.
//! That is the same split `obc_app::map_catalog` already documents — the board crate has no CI test
//! harness, so the *rules* live in shared crates and the board binds them. What this file proves is
//! that those rules, composed in the order the board composes them, turn a directory of shards into
//! a map — and that every way of getting the order wrong does not.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_ble::{HeldMagic, ObjectType, Receiver, SetPart, TransferControl, TransferResult, TransferStatus};
use obc_formats::io::ByteSource;
use obc_formats::obcs;
use obc_reader::{rgb565_to_rgb888, FullSetShards, MapCache, MapTables, MountedSet, Reader, SliceSource};
use obc_render::{MapRenderer, Viewport};
use obc_usb_host::set_transfer::{plan, send, LinkError, Options, Part, PlanError, Progress, SetLink, SetPlan};
use obcm_testkit::set::{matched_pair, SetFixture};
use obcm_testkit::{pack_line16, pack_poly16, seal, Style};

// ============================ the fixture ============================

const ASSEMBLY: (i32, i32, i32, i32) = (0, 0, 4000, 4000);
const COARSE_MPP: f32 = f32::INFINITY;
const FINE_MPP: f32 = 1.5;
const CHUNK: usize = 4096;
/// This board's ceiling — `sd::SD_SET_MAX_SHARDS`, the 11 shards #1026's handle budget bought.
const DEVICE_MAX_SHARDS: u8 = 11;
/// Free bytes a map-shaped upload must leave behind it — `sd::MAP_FREE_HEADROOM`. A card filled to
/// the last cluster strands the ride log and every sidecar, and the rider finds out mid-ride.
const MAP_FREE_HEADROOM: u64 = 16 << 20;

const STYLES: &[Style] = &[
    (1, 0, 0x07E0, 1, 1, false, None),
    (2, 0, 0xF800, 1, 1, false, None),
    (3, 0, 0x001F, 1, 1, false, None),
    (4, 0, 0xFFE0, 1, 1, false, None),
    (5, 0, 0x07FF, 3, 1, false, None),
];

fn color(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

fn fine_chunks() -> [Vec<u8>; 4] {
    let mut out = Vec::new();
    for (style, over) in [(1u8, (900i16, 900i16)), (2, (-900, 900)), (3, (900, -900)), (4, (-900, -900))] {
        let mut chunk = pack_poly16(style, 400, 400, &[(1200, 0), (0, 1200), (-1200, 0)]);
        chunk.extend_from_slice(&pack_line16(style, 1000, 1000, &[(over.0, 0), (0, over.1)]));
        out.push(seal(chunk, CHUNK));
    }
    [out[0].clone(), out[1].clone(), out[2].clone(), out[3].clone()]
}

fn coarse_chunk() -> Vec<u8> {
    let mut chunk = pack_line16(5, 200, 200, &[(3600, 0), (0, 3600), (-3600, 0), (0, -3600)]);
    chunk.extend_from_slice(&pack_line16(5, 200, 3800, &[(3600, -3600)]));
    seal(chunk, CHUNK)
}

/// A six-shard set (core + coarse + four geometry quadrants) and the monolithic file it was split
/// from, hand-built by `obcm-testkit` — an independent oracle, so the differential at the end of
/// the round trip means something.
fn pair() -> (Vec<u8>, SetFixture) {
    matched_pair(ASSEMBLY, STYLES, (COARSE_MPP, coarse_chunk(), CHUNK), (FINE_MPP, fine_chunks(), CHUNK))
}

/// A **second** six-shard set over the same assembly, with different content — and therefore
/// different shard byte lengths. Same shape on the wire (the descriptor carries a count and an
/// index, and both match), different bytes on the card, which is exactly what makes it the
/// interleave case a shard announce cannot see.
fn pair_variant() -> (Vec<u8>, SetFixture) {
    let mut chunk = pack_line16(5, 200, 200, &[(3600, 0), (0, 3600), (-3600, 0), (0, -3600)]);
    chunk.extend_from_slice(&pack_line16(5, 200, 3800, &[(3600, -3600)]));
    // The one difference: another polyline, so every byte count downstream of it moves.
    chunk.extend_from_slice(&pack_line16(5, 400, 400, &[(1200, 0), (0, 1200), (-1200, 0), (0, -1200)]));
    matched_pair(ASSEMBLY, STYLES, (COARSE_MPP, seal(chunk, CHUNK), CHUNK), (FINE_MPP, fine_chunks(), CHUNK))
}

/// Write a set to `dir` the way an assembler does — **with real SHA-256 digests**, because the
/// host's plan verifies them (`OBCA_Spec.md` §5.3 makes that the host's job, not the device's) and
/// `obcm-testkit` deliberately writes zeros.
fn write_set(dir: &Path, card_id: u16, fixture: &SetFixture) {
    use sha2::{Digest, Sha256};

    let digests: Vec<[u8; 32]> = fixture.shards.iter().map(|bytes| Sha256::digest(bytes).into()).collect();
    let mut manifest = obcs::parse(&fixture.manifest).expect("the testkit manifest is valid");
    let mut id_hash = Sha256::new();
    for digest in &digests {
        id_hash.update(digest);
    }
    // §5.2: the set id is the first 16 bytes of SHA-256 over the shard digests in index order.
    manifest.set_id.copy_from_slice(&id_hash.finalize()[..16]);
    let mut bytes = vec![0u8; manifest.encoded_len()];
    let len = obcs::serialize(&manifest, &digests, &mut bytes).expect("re-serialize with real digests");
    bytes.truncate(len);

    for (index, shard) in fixture.shards.iter().enumerate() {
        let name = obcs::shard_name(card_id, index).expect("a derived shard name");
        std::fs::write(dir.join(name.as_str()), shard).expect("write a shard");
    }
    let name = obcs::manifest_name(card_id).expect("a derived manifest name");
    std::fs::write(dir.join(name.as_str()), &bytes).expect("write the manifest");
}

fn tempdir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("obc-set-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

// ============================ the device under test ============================

/// The board's `set_reject_status`, mirrored. A second implementation of this mapping is exactly
/// what a companion app would write, so having one here is not duplication for its own sake — it is
/// the check that `obc_app::SetReject` carries enough to answer with.
fn reject_status(reject: obc_app::SetReject) -> TransferStatus {
    match reject {
        obc_app::SetReject::Part => TransferStatus::NotFound,
        obc_app::SetReject::Shards => TransferStatus::StorageFull,
        obc_app::SetReject::Mismatch | obc_app::SetReject::ManifestEarly | obc_app::SetReject::Length => {
            TransferStatus::Error
        }
    }
}

/// Which upload path owns the device's one streaming handle — the model of `sd::UploadOwner`. It
/// is what stops one transport's teardown closing the file the *other* transport is writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadOwner {
    /// `/routes/UPLOAD.TMP`.
    Temp,
    /// One file of a volume set — a shard, or the manifest.
    Set,
}

/// Where an interruption lands inside a file's stream. The adversarial points a set upload dies at
/// are not all "between transfers", and the ones inside a transfer are the ones that were never
/// exercised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interrupt {
    /// The **other** transport's link dropped this many payload bytes in: `ObjectStore::link_reset`
    /// runs — on the board it runs on either wire — and the set must be standing afterwards.
    BleReset(u64),
    /// The power went, this many payload bytes in. Nothing runs afterwards: no abort, no result,
    /// no delete. The card is exactly what the next boot finds.
    PowerCut(u64),
}

/// How a streamed file ended.
enum StreamOutcome {
    Done(Receiver, HeldMagic),
    /// The streaming handle went away under the write (`Storage::upload_append` → false). On the
    /// board that is a `discard_upload`, which for a set target deletes the whole set.
    AppendFailed,
    /// The device stopped existing mid-write.
    PowerCut,
}

/// The device's receive path over a directory instead of a FAT volume: the announce policy, the
/// held-back magic, the session, the commit checks, the abort, and the boot sweep — composed in the
/// order `link::transfer::classify_transfer` → `usb::data_plane::run_upload` → `ObjectStore` →
/// `sd::Storage` composes them on the board.
struct SimCard {
    root: PathBuf,
    session: Option<obc_app::SetUpload>,
    max_shards: u8,
    /// The one streaming handle and its owner — `Storage::open_upload`.
    open_upload: Option<(std::fs::File, UploadOwner)>,
    /// Free bytes the card reports, for the `Storage::card_free_bytes` guard. `None` = unknown,
    /// which is how the board treats a card it could not read a FAT32 FSInfo from.
    free_bytes: Option<u64>,
    /// Drop the link before the *n*-th `send_object` — the between-transfers kill.
    drop_before: Option<usize>,
    /// Interrupt the *n*-th `send_object` inside its stream.
    interrupt: Option<(usize, Interrupt)>,
    /// Write only the first two bytes of the manifest's magic at the commit — a four-byte write
    /// split by a power cut, which is a real shape and not a contrived one.
    tear_manifest_patch: bool,
    /// A file the card refuses to delete, to prove `delete_set`'s stop-if-the-manifest-survives
    /// rule is still there after the `NotFound` repair.
    undeletable: Option<String>,
    calls: usize,
}

impl SimCard {
    fn new(root: PathBuf) -> Self {
        SimCard {
            root,
            session: None,
            max_shards: DEVICE_MAX_SHARDS,
            open_upload: None,
            free_bytes: None,
            drop_before: None,
            interrupt: None,
            tear_manifest_patch: false,
            undeletable: None,
            calls: 0,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// `Storage::next_set_id_from_scan`: one past the highest id **any `MS` name on the card**
    /// mentions — manifests and shards alike, listed or not.
    ///
    /// Counting only *listed* sets was wrong in the one case it had to be right about: a set being
    /// hand-copied shards-first has no manifest yet, so it is listed nowhere, and minting its id
    /// would hand the next upload the rider's own files to clear.
    ///
    /// Note it can return `MAX_SET_ID + 1`, which has no derivable 8.3 name. That is the point:
    /// the exhaustion test is `> MAX_SET_ID`, so saturating *at* 999 would hand back an id a card
    /// holding `MS999` is already using — and since a set upload clears its id before writing to
    /// it, that would delete a good map to make room for a new one.
    fn next_set_id(&self) -> u16 {
        let mut next = 0u16;
        for entry in std::fs::read_dir(&self.root).expect("read the card root").flatten() {
            let name = entry.file_name().to_string_lossy().to_uppercase();
            let id = obcs::parse_manifest_name(name.as_bytes())
                .or_else(|| obcs::parse_shard_name(name.as_bytes()).map(|(id, _)| id));
            if let Some(id) = id {
                next = next.max(id + 1);
            }
        }
        next
    }

    /// `Storage::set_identity` + `set_shard_totals`: parse the manifest and check every shard it
    /// names is present at the recorded size, version **and header bbox**. `None` = *this is not a
    /// map*. There is no partial acceptance, and the bbox is part of the check because the board's
    /// is — a shard from a different set can match on length and version alone.
    fn scan_set(&self, id: u16) -> Option<obcs::SetManifest> {
        let name = obcs::manifest_name(id)?;
        let bytes = std::fs::read(self.path(name.as_str())).ok()?;
        let manifest = obcs::parse(&bytes).ok()?;
        for (index, shard) in manifest.shards().iter().enumerate() {
            let shard_name = obcs::shard_name(id, index)?;
            let file = std::fs::read(self.path(shard_name.as_str())).ok()?;
            if file.len() as u32 != shard.bytes || file.len() < obc_formats::obcm::HEADER_LEN {
                return None;
            }
            if file[0..4] != obc_formats::obcm::MAGIC || file[4] != manifest.obcm_version {
                return None;
            }
            // `Storage::shard_identity`: the 40-byte header's global bbox, at offsets 5, 9, 13, 17.
            let rd = |o: usize| i32::from_le_bytes([file[o], file[o + 1], file[o + 2], file[o + 3]]);
            let recorded = shard.bbox;
            if (rd(5), rd(9), rd(13), rd(17))
                != (recorded.min_lat, recorded.min_lon, recorded.max_lat, recorded.max_lon)
            {
                return None;
            }
        }
        Some(manifest)
    }

    /// `Storage::set_upload_begin`: clear anything under this id (§5.4's "delete the old manifest
    /// **before** overwriting any of its shards"), then write the zero-magic in-flight token.
    fn begin_set(&mut self, id: u16) {
        self.upload_close();
        self.delete_set(id);
        let name = obcs::manifest_name(id).expect("a derived manifest name");
        std::fs::write(self.path(name.as_str()), [0u8; 4]).expect("write the set token");
    }

    /// `Storage::delete_set`: `obcs::delete_plan`'s ordered list — manifest first, then every shard
    /// name to the cap — with the same stop-if-the-manifest-survives rule.
    ///
    /// **A manifest that is not there is not a manifest that survived**: `NotFound` continues, and
    /// only a delete that *failed against a file that is still there* stops the plan. Modelling the
    /// old behaviour with a `path.exists()` guard is what let the board's version — which has no
    /// such guard, because the card answers `NotFound` — bail out of every replace-clear on an id
    /// carrying shards and no manifest.
    fn delete_set(&self, id: u16) -> usize {
        let Some(plan) = obcs::delete_plan(id) else { return 0 };
        let mut removed = 0;
        for (step, derived) in plan.iter().enumerate() {
            let name = derived.as_str();
            if self.undeletable.as_deref() == Some(name) {
                if step == 0 {
                    return 0; // the manifest survives: its shards must not be touched
                }
                continue;
            }
            // An absent name is the ordinary case at every step, the manifest's included.
            if std::fs::remove_file(self.path(name)).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// `Storage::upload_abort`, reached from `ObjectStore::link_reset` — which runs on **either**
    /// transport's teardown. It closes the temp and only the temp: a map or a set streaming on the
    /// other wire keeps its handle (issue #1039).
    fn upload_abort(&mut self) {
        if matches!(self.open_upload, Some((_, owner)) if owner != UploadOwner::Temp) {
            return;
        }
        self.open_upload = None;
    }

    /// `Storage::upload_close`: flush and release, keeping the bytes.
    fn upload_close(&mut self) {
        self.open_upload = None;
    }

    /// `ObjectStore::link_reset` — a link dropped, on **either** wire. Deliberately does not touch
    /// the set: the cable's own teardown owns that.
    fn link_reset(&mut self) {
        self.upload_abort();
    }

    /// `ObjectStore::set_upload_abort` — the cable's teardown, and the `op=3` that reaches it.
    fn set_upload_abort(&mut self) {
        if let Some(session) = self.session.take() {
            if let Some((_, UploadOwner::Set)) = self.open_upload {
                self.open_upload = None;
            }
            self.delete_set(session.id());
        }
    }

    /// The USB endpoint going away (an unplug): `link_reset` **and** the set teardown beside it.
    fn cable_gone(&mut self) {
        self.link_reset();
        self.set_upload_abort();
    }

    /// `classify_transfer`'s `op=3` with nothing in flight, on the cable: discard any stray temp,
    /// and abandon the set staged between transfers.
    fn abort_between_transfers(&mut self) {
        self.upload_abort();
        self.set_upload_abort();
    }

    /// The boot sweep: `Storage::sweep_aborted_sets`. Returns how many files it reclaimed.
    fn sweep(&self) -> usize {
        let mut manifests: Vec<u16> = Vec::new();
        let mut shards: Vec<(String, u16)> = Vec::new();
        for entry in std::fs::read_dir(&self.root).expect("read the card root").flatten() {
            let name = entry.file_name().to_string_lossy().to_uppercase();
            if let Some(id) = obcs::parse_manifest_name(name.as_bytes()) {
                manifests.push(id);
            } else if let Some((id, _)) = obcs::parse_shard_name(name.as_bytes()) {
                shards.push((name, id));
            }
        }
        let mut swept = 0;
        for id in &manifests {
            let name = obcs::manifest_name(*id).expect("a derived manifest name");
            if obc_app::sweep_verdict(self.root_magic(name.as_str())) == obc_app::SweepVerdict::Reclaim {
                swept += self.delete_set(*id);
            }
        }
        for (name, id) in &shards {
            if manifests.contains(id) {
                continue;
            }
            if obc_app::orphan_shard_verdict(self.root_magic(name)) == obc_app::SweepVerdict::Reclaim
                && std::fs::remove_file(self.path(name)).is_ok()
            {
                swept += 1;
            }
        }
        swept
    }

    /// `Storage::root_magic`: the three answers a four-byte read can give, kept apart. "Shorter
    /// than four bytes" is a state only this device produces; "could not be read" is a bus glitch.
    fn root_magic(&self, name: &str) -> obc_app::RootMagic {
        match std::fs::read(self.path(name)) {
            Ok(bytes) if bytes.len() >= 4 => obc_app::RootMagic::Bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            Ok(_) => obc_app::RootMagic::Short,
            Err(_) => obc_app::RootMagic::Unreadable,
        }
    }

    fn magic_of(&self, name: &str) -> Option<[u8; 4]> {
        match self.root_magic(name) {
            obc_app::RootMagic::Bytes(magic) => Some(magic),
            _ => None,
        }
    }

    /// Stream one object into `name` with its leading four magic bytes withheld, exactly as
    /// `usb::data_plane::run_upload` does: the `Receiver`'s CRC sees every payload byte, only the
    /// *write* skips the held prefix, and every write goes through the store's **owned** handle so
    /// a teardown that closes it is felt here.
    fn stream(&mut self, name: &str, desc: &TransferControl, bytes: &mut dyn Read) -> StreamOutcome {
        let mut rx = Receiver::new(desc).expect("an upload descriptor");
        let mut held = HeldMagic::new();
        let mut file = std::fs::File::create(self.path(name)).expect("create the target");
        file.write_all(&[0u8; 4]).expect("the placeholder magic");
        self.open_upload = Some((file, UploadOwner::Set));

        let interrupt = self.interrupt.filter(|(call, _)| *call == self.calls).map(|(_, kind)| kind);
        let mut streamed = 0u64;
        let mut buf = [0u8; 4096];
        while !rx.is_complete() {
            let n = bytes.read(&mut buf).expect("read the source");
            if n == 0 {
                break;
            }
            let consumed = rx.push(&buf[..n]);
            let write = held.feed(&buf[..consumed]);
            let reached = |at: u64| streamed < at && streamed + consumed as u64 >= at;
            match interrupt {
                // The phone dropped off the radio while the cable is mid-shard. This is the whole
                // of what the board runs on that event, and the set must survive it.
                Some(Interrupt::BleReset(at)) if reached(at) => self.link_reset(),
                Some(Interrupt::PowerCut(at)) if reached(at) => return StreamOutcome::PowerCut,
                _ => {}
            }
            streamed += consumed as u64;
            if !self.upload_append(write) {
                return StreamOutcome::AppendFailed;
            }
        }
        self.upload_close();
        StreamOutcome::Done(rx, held)
    }

    /// `Storage::upload_append`: false = the handle is gone, which the data plane turns into a
    /// discard of the whole upload.
    fn upload_append(&mut self, bytes: &[u8]) -> bool {
        match &mut self.open_upload {
            Some((file, _)) => file.write_all(bytes).is_ok(),
            None => false,
        }
    }

    /// Patch a committed file's real magic over the placeholder — the commit point. `torn` writes
    /// only half of it, which is what a power cut inside the four-byte write leaves.
    fn patch_magic(&self, name: &str, magic: [u8; 4], torn: bool) {
        use std::io::Seek;
        let mut file = std::fs::OpenOptions::new().write(true).open(self.path(name)).expect("reopen to commit");
        file.seek(std::io::SeekFrom::Start(0)).expect("seek");
        let written = if torn { &magic[..2] } else { &magic[..] };
        file.write_all(written).expect("the commit write");
    }
}

impl SetLink for SimCard {
    fn send_object(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> Result<TransferResult, LinkError> {
        self.calls += 1;
        if self.drop_before == Some(self.calls) {
            return Err(LinkError("the cable was pulled".into()));
        }
        match desc.ty {
            ObjectType::MapShard => self.recv_shard(desc, bytes),
            ObjectType::MapSet => self.recv_manifest(desc, bytes),
            other => panic!("a set transfer offered {other:?}"),
        }
    }
}

impl SimCard {
    /// `ObjectStore::set_shard_open` → `set_shard_begin` → `set_shard_finish`.
    fn recv_shard(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> Result<TransferResult, LinkError> {
        let refuse = |status| Ok(TransferResult::new(desc.object_id, status, 0));
        let Some(part) = SetPart::decode(desc.object_id) else {
            return refuse(TransferStatus::NotFound);
        };
        let fresh = match obc_app::shard_announce(self.session.as_ref(), part.shard_count, part.index, self.max_shards)
        {
            Ok(fresh) => fresh,
            Err(reject) => return refuse(reject_status(reject)),
        };
        // The id-space refusal is an **announce** refusal, and answers `storageFull` like the shard
        // ceiling does: a catalog that cannot take another entry, refused before a byte streams
        // rather than at the first one with a red storage-failed card.
        if fresh && self.next_set_id() > obcs::MAX_SET_ID {
            return refuse(TransferStatus::StorageFull);
        }
        if desc.total_len < obc_formats::obcm::HEADER_LEN as u32 {
            return refuse(TransferStatus::Error);
        }
        // `Storage::card_free_bytes` + `MAP_FREE_HEADROOM`: per file, because the device is not
        // told the set's total until the manifest, which by §5.4 is last.
        if let Some(free) = self.free_bytes {
            if desc.total_len as u64 + MAP_FREE_HEADROOM > free {
                return refuse(TransferStatus::StorageFull);
            }
        }
        if fresh {
            let id = self.next_set_id();
            self.begin_set(id);
            self.session = Some(obc_app::SetUpload::new(id, part.shard_count));
        }
        let id = self.session.as_ref().expect("a session is open").id();
        let name = obcs::shard_name(id, part.index as usize).expect("a derived shard name").as_str().to_string();

        let (rx, held) = match self.stream(&name, desc, bytes) {
            StreamOutcome::Done(rx, held) => (rx, held),
            StreamOutcome::PowerCut => return Err(LinkError("the power went mid-shard".into())),
            // `usb::data_plane::discard_upload` for a shard target: the whole set goes.
            StreamOutcome::AppendFailed => {
                self.set_upload_abort();
                return refuse(TransferStatus::Error);
            }
        };
        let outcome = match rx.outcome() {
            Some(o) if o.status == TransferStatus::Committed => o,
            other => {
                // A failed shard drops itself and leaves the session standing — the host re-sends
                // one file, not the set.
                let _ = std::fs::remove_file(self.path(&name));
                return refuse(other.map_or(TransferStatus::Error, |o| o.status));
            }
        };
        let magic = held.take().expect("an OBCM-sized object carries a magic");
        // `Storage::set_shard_commit`: the header must validate with the real magic spliced in,
        // before it is written.
        let mut header = std::fs::read(self.path(&name)).expect("read back");
        header[0..4].copy_from_slice(&magic);
        if obc_formats::obcm::validate_header_prefix(&header).is_err() {
            let _ = std::fs::remove_file(self.path(&name));
            return refuse(TransferStatus::Error);
        }
        self.patch_magic(&name, magic, false);
        if let Some(session) = &mut self.session {
            session.mark(part.index);
        }
        Ok(TransferResult::new(part.encode(), TransferStatus::Committed, outcome.committed_offset))
    }

    /// `ObjectStore::set_manifest_open` → `set_manifest_begin` → `set_manifest_finish`, i.e. the
    /// place §5.4's manifest-last rule is *enforced*.
    fn recv_manifest(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> Result<TransferResult, LinkError> {
        let refuse = |status| Ok(TransferResult::new(desc.object_id, status, 0));
        if desc.object_id != TransferControl::NEW_OBJECT_ID {
            return refuse(TransferStatus::NotFound);
        }
        if let Err(reject) = obc_app::manifest_announce(self.session.as_ref(), desc.total_len) {
            return refuse(reject_status(reject));
        }
        let id = self.session.as_ref().expect("the announce proved a session").id();
        let name = obcs::manifest_name(id).expect("a derived manifest name").as_str().to_string();

        let (rx, held) = match self.stream(&name, desc, bytes) {
            StreamOutcome::Done(rx, held) => (rx, held),
            StreamOutcome::PowerCut => return Err(LinkError("the power went mid-manifest".into())),
            StreamOutcome::AppendFailed => {
                self.set_upload_abort();
                return refuse(TransferStatus::Error);
            }
        };
        self.session = None;
        let outcome = match rx.outcome() {
            Some(o) if o.status == TransferStatus::Committed => o,
            other => {
                self.delete_set(id);
                return refuse(other.map_or(TransferStatus::Error, |o| o.status));
            }
        };
        let magic = held.take().expect("a manifest carries a magic");
        // `Storage::validate_committed_manifest`: read back with the magic spliced in **in memory**,
        // parse against §5.3, and check it against the shards actually on the card — and only then
        // write those four bytes. Splicing the magic onto the card first would make the file a
        // manifest for as long as the validation takes, which is the one thing the held-back magic
        // exists to prevent.
        let mut manifest_bytes = std::fs::read(self.path(&name)).expect("read back");
        manifest_bytes[0..4].copy_from_slice(&magic);
        if !self.manifest_describes_the_card(id, &manifest_bytes) {
            self.delete_set(id);
            return refuse(TransferStatus::Error);
        }
        self.patch_magic(&name, magic, self.tear_manifest_patch);
        Ok(TransferResult::new(id, TransferStatus::Committed, outcome.committed_offset))
    }

    /// `Storage::validate_committed_manifest`'s in-memory half: does this manifest describe the
    /// shards on the card? Same checks as [`scan_set`](Self::scan_set), against bytes that are not
    /// on the card yet.
    fn manifest_describes_the_card(&self, id: u16, bytes: &[u8]) -> bool {
        let Ok(manifest) = obcs::parse(bytes) else { return false };
        if manifest.encoded_len() != bytes.len() {
            return false;
        }
        for (index, shard) in manifest.shards().iter().enumerate() {
            let Some(shard_name) = obcs::shard_name(id, index) else { return false };
            let Ok(file) = std::fs::read(self.path(shard_name.as_str())) else { return false };
            if file.len() as u32 != shard.bytes || file.len() < obc_formats::obcm::HEADER_LEN {
                return false;
            }
            if file[0..4] != obc_formats::obcm::MAGIC || file[4] != manifest.obcm_version {
                return false;
            }
            let rd = |o: usize| i32::from_le_bytes([file[o], file[o + 1], file[o + 2], file[o + 3]]);
            if (rd(5), rd(9), rd(13), rd(17))
                != (shard.bbox.min_lat, shard.bbox.min_lon, shard.bbox.max_lat, shard.bbox.max_lon)
            {
                return false;
            }
        }
        true
    }
}

// ============================ rendering the result ============================

struct Buf {
    px: Vec<Rgb888>,
    w: u32,
    h: u32,
}

impl Buf {
    fn new(w: u32, h: u32) -> Self {
        Buf { px: vec![Rgb888::BLACK; (w * h) as usize], w, h }
    }
}

impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }
}

impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Self::Color>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && (p.x as u32) < self.w && (p.y as u32) < self.h {
                self.px[(p.y as u32 * self.w + p.x as u32) as usize] = c;
            }
        }
        Ok(())
    }
}

fn viewport() -> Viewport {
    Viewport::new(220.0, 220.0, 2000, 2000, 0.075)
}

fn render_monolith(bytes: &[u8]) -> Buf {
    let mut buf = Buf::new(220, 220);
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("the monolith parses");
    let reader = Reader::new(&src, &tables, &cache);
    MapRenderer::new().render(&mut buf, &reader, &viewport(), Rgb888::BLACK, color);
    buf
}

/// Mount the set **off the card the transfer wrote** and render it. This is the assertion that the
/// bytes that landed are a map, not merely that a transfer reported `committed`.
fn render_card_set(card: &SimCard, id: u16) -> Buf {
    let manifest_name = obcs::manifest_name(id).expect("a derived manifest name");
    let manifest_bytes = std::fs::read(card.path(manifest_name.as_str())).expect("read the manifest");
    let manifest = obcs::parse(&manifest_bytes).expect("the committed manifest parses");
    let shard_bytes: Vec<Vec<u8>> = (0..manifest.shard_count())
        .map(|index| {
            let name = obcs::shard_name(id, index).expect("a derived shard name");
            std::fs::read(card.path(name.as_str())).expect("read a shard")
        })
        .collect();
    let sources: Vec<SliceSource> = shard_bytes.iter().map(|b| SliceSource(b)).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let core = MapTables::parse(&sources[manifest.core_shard()]).expect("the core parses");
    let cache = MapCache::new();
    let mut store = FullSetShards::new();
    let set = MountedSet::mount(&mut store, &manifest, &refs, &core, &cache).expect("the received set mounts");
    let mut buf = Buf::new(220, 220);
    MapRenderer::new().render(&mut buf, &set, &viewport(), Rgb888::BLACK, color);
    buf
}

fn planned(dir: &Path, card_id: u16) -> SetPlan {
    plan(dir, card_id).expect("the fixture set plans")
}

fn no_progress(_: Progress) {}

// ============================ the tests ============================

/// **The round trip.** Assemble a set, plan it, send it over the device's own receive logic, and
/// then mount and render what landed — against the monolith the set was split from, pixel for
/// pixel. Everything in between is the real thing: the descriptors, the CRCs, the held-back magic,
/// the manifest-last order.
#[test]
fn a_set_sent_in_order_lands_as_a_map_that_renders() {
    let host = tempdir("roundtrip-host");
    let card = tempdir("roundtrip-card");
    let (monolith, fixture) = pair();
    write_set(&host, 3, &fixture);

    let plan = planned(&host, 3);
    assert_eq!(plan.files.len(), fixture.shards.len() + 1);
    assert_eq!(plan.manifest().part, Part::Manifest, "the plan ends with the manifest");

    let mut device = SimCard::new(card.clone());
    let sent = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the set transfers");
    assert_eq!(sent.bytes, plan.total_bytes());

    // The device minted its own id — filenames are derived, not carried (§5.2) — and the card holds
    // exactly the derived names.
    let id = sent.device_set_id;
    assert!(device.exists(obcs::manifest_name(id).unwrap().as_str()));
    for index in 0..fixture.shards.len() {
        assert!(device.exists(obcs::shard_name(id, index).unwrap().as_str()), "shard {index} landed");
    }
    // The manifest committed, so its magic is in — the set is a map to every reader.
    assert_eq!(device.magic_of(obcs::manifest_name(id).unwrap().as_str()), Some(obcs::MAGIC));
    assert!(device.scan_set(id).is_some(), "the boot scan lists it (OBCA §5.3)");

    let from_card = render_card_set(&device, id);
    let from_monolith = render_monolith(&monolith);
    assert!(from_card.px.iter().any(|&p| p != Rgb888::BLACK), "an empty frame would prove nothing");
    assert_eq!(from_card.px, from_monolith.px, "the received set renders as the map it was split from");

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// **Manifest-first is refused, not trusted** (`OBCA_Spec.md` §5.4). The device answers before a
/// byte streams and the card stays empty — no token, no manifest, nothing for a later scan to
/// half-believe.
#[test]
fn a_manifest_sent_before_its_shards_is_refused_and_writes_nothing() {
    let host = tempdir("manifest-first-host");
    let card = tempdir("manifest-first-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    let manifest = plan.manifest();
    let mut bytes = std::fs::File::open(&manifest.path).expect("open the manifest");
    let result = device.send_object(&manifest.descriptor(), &mut bytes).expect("the link is fine");
    assert_eq!(result.status, TransferStatus::Error, "no set is in flight, so there is nothing this is the last of");
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0, "nothing was written");

    // And with a set *partly* uploaded it is still refused — a hole in the middle is not "last".
    let first = &plan.files[0];
    let mut shard = std::fs::File::open(&first.path).expect("open shard 0");
    assert_eq!(
        device.send_object(&first.descriptor(), &mut shard).expect("the link is fine").status,
        TransferStatus::Committed
    );
    let mut bytes = std::fs::File::open(&manifest.path).expect("open the manifest");
    let result = device.send_object(&manifest.descriptor(), &mut bytes).expect("the link is fine");
    assert_eq!(result.status, TransferStatus::Error, "one shard of six is not every shard");
    // The token is still zero-magic: the refusal did not write a manifest over it.
    let id = device.session.as_ref().expect("the session survives a refused manifest").id();
    assert_eq!(device.magic_of(obcs::manifest_name(id).unwrap().as_str()), Some([0, 0, 0, 0]));

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// **Killed mid-set.** After shard `k` the link drops: the card must hold no mountable map, the
/// next scan must refuse cleanly rather than mount a half-set, and the cleanup must reclaim every
/// file — the shards already committed included.
#[test]
fn a_set_killed_mid_upload_leaves_no_map_and_is_reclaimed() {
    let host = tempdir("abort-host");
    let card = tempdir("abort-card");
    let (_, fixture) = pair();
    write_set(&host, 5, &fixture);
    let plan = planned(&host, 5);

    // Drop the cable before the third file — two shards committed, four to go.
    let mut device = SimCard::new(card.clone());
    device.drop_before = Some(3);
    let err = send(&mut device, &plan, Options::default(), &mut no_progress).expect_err("the link dies");
    assert!(matches!(err, obc_usb_host::set_transfer::SendError::Link(_)), "a dropped cable, not a refusal");

    let id = device.session.as_ref().expect("the session is still open").id();
    assert!(device.exists(obcs::shard_name(id, 0).unwrap().as_str()), "two shards did land");
    assert!(device.scan_set(id).is_none(), "…and the card still holds no map (OBCA §5.4)");
    assert_eq!(
        device.magic_of(obcs::manifest_name(id).unwrap().as_str()),
        Some([0, 0, 0, 0]),
        "the manifest is the zero-magic in-flight token, which is what makes this reclaimable"
    );

    // What the data plane does when it sees the *cable* go: `link_reset` plus the set teardown.
    device.cable_gone();
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0, "the whole set is gone, shards included");

    // …and had the power gone instead of the cable, the boot sweep is the same reclaim.
    let mut device = SimCard::new(card.clone());
    device.drop_before = Some(3);
    let _ = send(&mut device, &plan, Options::default(), &mut no_progress);
    let power_cut = SimCard::new(card.clone()); // a fresh boot: no session, only the card
    assert!(std::fs::read_dir(&card).unwrap().count() > 0, "the corpse is there before the sweep");
    assert!(power_cut.sweep() > 0, "the sweep reclaims it");
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0);

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// **The grace the sweep grants a rider.** A set being copied over a card reader is shards-first
/// with no manifest yet — exactly the "orphan" shape §5.4 says a writer MAY delete. The sweep must
/// not, because those shards have their magic and the manifest is probably still coming; deleting
/// them would destroy a map minutes from working.
#[test]
fn a_set_being_copied_by_hand_survives_the_boot_sweep() {
    let card = tempdir("grace-card");
    let (_, fixture) = pair();
    // Mid-copy: every shard complete, the manifest not copied yet.
    for (index, shard) in fixture.shards.iter().enumerate() {
        let name = obcs::shard_name(9, index).expect("a derived shard name");
        std::fs::write(card.join(name.as_str()), shard).expect("copy a shard");
    }
    let before = std::fs::read_dir(&card).unwrap().count();

    let device = SimCard::new(card.clone());
    assert_eq!(device.sweep(), 0, "a complete orphan shard is never the sweep's to take");
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), before);

    // Finish the copy and the set is simply a map, with no sweep having touched it.
    write_set(&card, 9, &fixture);
    assert!(device.scan_set(9).is_some());
    assert_eq!(device.sweep(), 0, "a valid set is not debris either");

    // The residue that *is* the sweep's: a zero-magic orphan, i.e. a shard stream that never
    // committed and whose set token is already gone.
    std::fs::write(card.join("MS4S00.OBM"), [0u8; 4]).expect("write an orphan");
    assert_eq!(device.sweep(), 1);
    assert!(!card.join("MS4S00.OBM").exists());

    let _ = std::fs::remove_dir_all(&card);
}

/// A set larger than this board can hold handles for is refused at the **first** shard — the whole
/// reason the shard count rides every descriptor rather than only the manifest. Refusing here costs
/// nothing; refusing at the manifest would cost the entire upload.
#[test]
fn a_set_past_the_device_ceiling_is_refused_before_any_bytes() {
    let host = tempdir("ceiling-host");
    let card = tempdir("ceiling-card");
    let (_, fixture) = pair();
    write_set(&host, 2, &fixture);
    let plan = planned(&host, 2);

    let mut device = SimCard::new(card.clone());
    device.max_shards = 3; // a board with a much smaller handle budget

    let first = &plan.files[0];
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    // Six shards against a ceiling of three.
    let result = device.send_object(&first.descriptor(), &mut bytes).expect("the link is fine");
    assert_eq!(result.status, TransferStatus::StorageFull);
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0, "not one byte, and no token");

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// The top of the id namespace. `MS999` is the last set `OBCA_Spec.md` §5.2's 8.3 names can
/// express, so a card already holding one leaves nowhere to put a new set — and the upload must be
/// **refused** rather than handed id 999 again. A set upload clears its id before it writes to it
/// (§5.4's replace rule), so re-issuing the top id would delete the rider's map to make room for
/// the one replacing it, and a failed transfer would leave them with neither.
#[test]
fn a_full_set_id_namespace_refuses_rather_than_reusing_the_last_id() {
    let host = tempdir("exhaust-host");
    let card = tempdir("exhaust-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    // The card already holds the highest set the naming scheme can express.
    write_set(&card, obcs::MAX_SET_ID, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    assert!(device.scan_set(obcs::MAX_SET_ID).is_some(), "MS999 is a real map on this card");
    assert_eq!(device.next_set_id(), obcs::MAX_SET_ID + 1, "one past the last derivable name");

    let first = &plan.files[0];
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    let result = device.send_object(&first.descriptor(), &mut bytes).expect("the link is fine");
    assert_eq!(
        result.status,
        TransferStatus::StorageFull,
        "there is no id left to mint, and that is a catalog refusal at the announce — not a storage failure \
         discovered at the first byte"
    );
    assert!(device.session.is_none(), "and no session was opened");
    assert!(device.scan_set(obcs::MAX_SET_ID).is_some(), "the map that was already there is untouched");

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// The host's own §5.3 obligation, checked before a device is involved at all: a shard that does
/// not match the SHA-256 the manifest records is refused by [`plan`], so a corrupted download never
/// becomes a multi-gigabyte upload that fails at the end.
#[test]
fn a_shard_that_fails_its_digest_is_refused_by_the_host() {
    let host = tempdir("digest-host");
    let (_, fixture) = pair();
    write_set(&host, 7, &fixture);

    let path = host.join(obcs::shard_name(7, 1).unwrap().as_str());
    let mut bytes = std::fs::read(&path).expect("read a shard");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF; // same length, different content — only the digest can see this
    std::fs::write(&path, &bytes).expect("corrupt it");

    match plan(&host, 7) {
        Err(PlanError::ShardDigest { filename }) => assert_eq!(filename, "MS7S01.OBM"),
        other => panic!("expected a digest refusal, got {other:?}"),
    }

    // …and a truncated one is named by the cheaper check.
    write_set(&host, 7, &fixture);
    let path = host.join(obcs::shard_name(7, 0).unwrap().as_str());
    let bytes = std::fs::read(&path).expect("read a shard");
    std::fs::write(&path, &bytes[..bytes.len() - 1]).expect("truncate it");
    assert!(matches!(plan(&host, 7), Err(PlanError::ShardSize { .. })));

    let _ = std::fs::remove_dir_all(&host);
}

/// A set replacing one already on the card: §5.4's "delete the old manifest **before** overwriting
/// any of its shards". The window in which two sets' shards coexist must never have a manifest
/// pointing into it.
#[test]
fn uploading_a_second_set_never_leaves_a_manifest_over_mixed_shards() {
    let host = tempdir("replace-host");
    let card = tempdir("replace-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    let first = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the first set transfers");
    assert!(device.scan_set(first.device_set_id).is_some());

    // The second upload mints the next id, so the first set is untouched until the boot that
    // retires it (`obc_app::newest_set` + `is_superseded_upload`, wired in #1031).
    let second = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the second set transfers");
    assert_ne!(second.device_set_id, first.device_set_id, "a fresh id, so the live map is never overwritten");
    assert!(device.scan_set(first.device_set_id).is_some(), "both sets are on the card until the next boot");
    assert!(device.scan_set(second.device_set_id).is_some());

    // Re-uploading under an id whose set is torn reuses that id, and clears it first.
    device.begin_set(second.device_set_id);
    assert_eq!(
        device.magic_of(obcs::manifest_name(second.device_set_id).unwrap().as_str()),
        Some([0, 0, 0, 0]),
        "the old manifest is gone before any shard is overwritten"
    );
    assert!(device.scan_set(second.device_set_id).is_none(), "and the set stops being a map at that instant");

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

// ============================ the fix round (review of #1040) ============================
//
// Everything below is a rule the first cut got wrong. Each test is written so it fails against the
// code as it was, which is the only kind of regression test worth the lines.

/// **H1 — a BLE disconnect must not kill a USB set upload.** The teardown that runs when a phone
/// walks out of range is transport-blind by design (it also runs on the cable), and it used to
/// close whatever storage handle was open. Mid-shard, that handle is the cable's: the next append
/// failed, and the failure path deletes the whole set.
///
/// The fix is ownership — the handle knows which path opened it, and the temp path's abort leaves a
/// map's or a set's alone. So the phone can drop at any byte of any shard and the upload finishes.
#[test]
fn a_ble_disconnect_mid_shard_leaves_the_set_staged_and_resumable() {
    let host = tempdir("bledrop-host");
    let card = tempdir("bledrop-card");
    let (monolith, fixture) = pair();
    write_set(&host, 4, &fixture);
    let plan = planned(&host, 4);

    let mut device = SimCard::new(card.clone());
    // Half-way through the third file, i.e. deep inside a shard's stream.
    let mid = plan.files[2].len / 2;
    device.interrupt = Some((3, Interrupt::BleReset(mid)));

    let sent = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the cable keeps its transfer");
    let id = sent.device_set_id;
    assert!(device.scan_set(id).is_some(), "the set committed despite the radio dropping mid-shard");
    assert_eq!(render_card_set(&device, id).px, render_monolith(&monolith).px, "…and it is the map it should be");

    // And between transfers, the same teardown leaves the session and its staged shards standing.
    let card2 = tempdir("bledrop-card2");
    let mut device = SimCard::new(card2.clone());
    let first = &plan.files[0];
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    assert_eq!(
        device.send_object(&first.descriptor(), &mut bytes).expect("the link is fine").status,
        TransferStatus::Committed
    );
    device.link_reset();
    let staged = device.session.as_ref().expect("the set is still in flight").id();
    assert!(device.exists(obcs::shard_name(staged, 0).unwrap().as_str()), "and its shard is still there");

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
    let _ = std::fs::remove_dir_all(&card2);
}

/// **H2 — §5.4's replace-clear must clear.** An id can carry shards with no manifest naming them
/// (a delete whose first step landed and whose rest did not, a sweep interrupted). The clear that
/// opens a set upload treated "there is no manifest to delete" as "the manifest could not be
/// deleted" and stopped, so those shards survived into the next set written under that id.
#[test]
fn a_replace_clear_removes_the_shards_no_manifest_names() {
    let card = tempdir("clear-card");
    let (_, fixture) = pair();
    for (index, shard) in fixture.shards.iter().enumerate() {
        let name = obcs::shard_name(2, index).expect("a derived shard name");
        std::fs::write(card.join(name.as_str()), shard).expect("leave a shard behind");
    }
    assert!(!card.join("MS2.OBS").exists(), "and no manifest names them");

    let mut device = SimCard::new(card.clone());
    device.begin_set(2);

    for index in 0..fixture.shards.len() {
        let name = obcs::shard_name(2, index).expect("a derived shard name");
        assert!(!device.exists(name.as_str()), "shard {index} of the previous occupant is gone");
    }
    assert_eq!(device.magic_of("MS2.OBS"), Some([0, 0, 0, 0]), "and the id now holds only the fresh token");

    let _ = std::fs::remove_dir_all(&card);
}

/// …and **why** that matters: a valid manifest shields every shard of its id from the orphan sweep,
/// including ones it does not name. A clear that stopped short therefore left dead files nothing on
/// the device could ever reclaim or explain — not a leak a boot fixes, a permanent one.
#[test]
fn shards_a_manifest_does_not_name_are_invisible_to_the_sweep() {
    let card = tempdir("stranded-card");
    let (_, fixture) = pair();
    write_set(&card, 0, &fixture); // a real six-shard set at id 0
                                   // The tail a bigger predecessor would have left behind: index 6, named by nothing.
    let stray = obcs::shard_name(0, 6).expect("a derived shard name");
    std::fs::write(card.join(stray.as_str()), &fixture.shards[0]).expect("strand a shard");

    let device = SimCard::new(card.clone());
    assert!(device.scan_set(0).is_some(), "the set is a perfectly good map");
    assert_eq!(device.sweep(), 0, "and the sweep will not touch its id");
    assert!(device.exists(stray.as_str()), "so the stray is permanent — the clear is the only thing that can take it");

    let _ = std::fs::remove_dir_all(&card);
}

/// The guard the `NotFound` repair must **not** have removed: a manifest that is still there after
/// a failed delete stops the plan, because deleting its shards under it would leave a manifest
/// pointing at files that are gone — the one state §5.4's ordering exists to make impossible.
#[test]
fn a_manifest_that_will_not_delete_still_stops_the_clear() {
    let card = tempdir("stubborn-card");
    let (_, fixture) = pair();
    write_set(&card, 1, &fixture);

    let mut device = SimCard::new(card.clone());
    device.undeletable = Some("MS1.OBS".to_string());
    assert_eq!(device.delete_set(1), 0, "nothing was removed");
    assert!(device.scan_set(1).is_some(), "and the set is intact, manifest and every shard");

    let _ = std::fs::remove_dir_all(&card);
}

/// **M3 — a token too short to hold a magic.** The `.OBS` is created and *then* written, so a power
/// cut in that gap leaves a zero-byte manifest. Reading it gave "no magic", which was folded in
/// with "could not read the file" and kept forever — while its name went on shielding the set's
/// shards from the orphan pass. Gigabytes, invisible, permanent.
#[test]
fn a_manifest_too_short_to_hold_a_magic_is_reclaimed_at_the_next_boot() {
    let card = tempdir("shorttoken-card");
    let (_, fixture) = pair();
    for (index, shard) in fixture.shards.iter().enumerate() {
        let name = obcs::shard_name(6, index).expect("a derived shard name");
        std::fs::write(card.join(name.as_str()), shard).expect("a landed shard");
    }
    std::fs::write(card.join("MS6.OBS"), []).expect("the token's create without its write");

    let device = SimCard::new(card.clone());
    assert!(device.scan_set(6).is_none(), "it is no map — and nothing else could ever say so");
    assert_eq!(device.sweep(), fixture.shards.len() + 1, "the boot reclaims the manifest and every shard");
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0);

    let _ = std::fs::remove_dir_all(&card);
}

/// **M3 — a magic patch that tore.** The commit is one four-byte write, and one write is not one
/// sector: a power cut inside it leaves `OB\0\0`. The set is invisible to every reader either way,
/// so keeping it froze the whole set exactly as a zero-magic token would have.
#[test]
fn a_half_patched_manifest_magic_is_reclaimed_at_the_next_boot() {
    let host = tempdir("tornpatch-host");
    let card = tempdir("tornpatch-card");
    let (_, fixture) = pair();
    write_set(&host, 8, &fixture);
    let plan = planned(&host, 8);

    let mut device = SimCard::new(card.clone());
    device.tear_manifest_patch = true;
    let sent = send(&mut device, &plan, Options::default(), &mut no_progress).expect("every byte arrived");
    let id = sent.device_set_id;

    let magic = device.magic_of(obcs::manifest_name(id).unwrap().as_str()).expect("four bytes are there");
    assert_eq!(&magic[..2], &obcs::MAGIC[..2]);
    assert_eq!(&magic[2..], &[0, 0], "and the rest never landed");
    assert!(device.scan_set(id).is_none(), "so it is not a map");

    let fresh_boot = SimCard::new(card.clone());
    assert_eq!(fresh_boot.sweep(), fixture.shards.len() + 1, "the boot reclaims the set whole");
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0);

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// **M4 — a staged set can be abandoned.** A set lives across several descriptors, so `op=3` lands
/// *between* them, where nothing is in flight to abort. The host was told "aborted" while the
/// gigabytes stayed staged and the session went on refusing every differently-shaped set until the
/// cable was pulled.
#[test]
fn an_abort_between_transfers_discards_the_staged_set() {
    let host = tempdir("abandon-host");
    let card = tempdir("abandon-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    let first = &plan.files[0];
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    assert_eq!(
        device.send_object(&first.descriptor(), &mut bytes).expect("the link is fine").status,
        TransferStatus::Committed
    );
    assert!(device.session.is_some(), "a set is staged");

    device.abort_between_transfers();
    assert!(device.session.is_none(), "the session is closed");
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0, "and the card is clean, immediately");

    // …and the link is usable straight afterwards: a whole set goes up on the same connection.
    let sent = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the set transfers");
    assert!(device.scan_set(sent.device_set_id).is_some());

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// The other half of the same rule: while a set **is** staged, a differently-shaped one is refused
/// rather than mixed in — and the abort is what makes the link usable again without an unplug.
#[test]
fn a_second_set_offered_while_one_is_staged_is_refused_until_it_is_abandoned() {
    let host = tempdir("second-host");
    let card = tempdir("second-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    let first = &plan.files[0];
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    let _ = device.send_object(&first.descriptor(), &mut bytes).expect("the link is fine");

    // Shard 0 of a *three*-shard set, mid-six-shard-set.
    let mut other = first.descriptor();
    other.object_id = SetPart { shard_count: 3, index: 0 }.encode();
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    assert_eq!(
        device.send_object(&other, &mut bytes).expect("the link is fine").status,
        TransferStatus::Error,
        "a different shard count is a mismatch, not a second session"
    );

    device.abort_between_transfers();
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    assert_eq!(
        device.send_object(&other, &mut bytes).expect("the link is fine").status,
        TransferStatus::Committed,
        "and after the abandon it is simply the first shard of a new set"
    );

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// **M5 — what the announce cannot see, and what catches it anyway.** Two sets with the *same*
/// shard count are indistinguishable at a shard announce: the descriptor carries a count and an
/// index, and neither identifies a set. The spec now says so. What refuses the mix is the manifest
/// commit, which checks every shard against the manifest's own record of it — later than an
/// announce, but still before anything is a map, and with the whole set deleted behind it.
#[test]
fn two_same_count_sets_are_caught_at_the_manifest_not_the_announce() {
    let host_a = tempdir("mixa-host");
    let host_b = tempdir("mixb-host");
    let card = tempdir("mix-card");
    let (_, fixture_a) = pair();
    let (_, fixture_b) = pair_variant();
    assert_eq!(fixture_a.shards.len(), fixture_b.shards.len(), "same shape, different bytes");
    assert_ne!(fixture_a.shards[1].len(), fixture_b.shards[1].len());
    write_set(&host_a, 1, &fixture_a);
    write_set(&host_b, 1, &fixture_b);
    let plan_a = planned(&host_a, 1);
    let plan_b = planned(&host_b, 1);

    let mut device = SimCard::new(card.clone());
    for (index, file) in plan_a.files.iter().enumerate() {
        if file.part == Part::Manifest {
            break;
        }
        // Every shard from set A except one, which comes from set B — the shape a host that
        // switched sets mid-transfer produces.
        let source = if index == 1 { &plan_b.files[1] } else { file };
        let mut bytes = std::fs::File::open(&source.path).expect("open a shard");
        assert_eq!(
            device.send_object(&source.descriptor(), &mut bytes).expect("the link is fine").status,
            TransferStatus::Committed,
            "the announce accepts it — there is nothing in the descriptor that could refuse it"
        );
    }

    let manifest = plan_a.manifest();
    let mut bytes = std::fs::File::open(&manifest.path).expect("open the manifest");
    let result = device.send_object(&manifest.descriptor(), &mut bytes).expect("the link is fine");
    assert_eq!(result.status, TransferStatus::Error, "the commit checks every shard against the manifest");
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0, "and a set that is not a map is not left half-there");

    let _ = std::fs::remove_dir_all(&host_a);
    let _ = std::fs::remove_dir_all(&host_b);
    let _ = std::fs::remove_dir_all(&card);
}

/// **M6 — the allocator and the rider mid-copy.** A set being hand-copied shards-first is listed
/// nowhere (it has no manifest yet), and it is the exact shape the sweep goes out of its way to
/// spare. An allocator that counted only *listed* sets minted its id anyway — and the clear that
/// opens an upload would then delete the rider's half-copied map to make room for a new one.
#[test]
fn a_hand_copy_in_progress_never_has_its_id_minted_over() {
    let host = tempdir("midcopy-host");
    let card = tempdir("midcopy-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    // The rider's copy: every shard at id 0, the manifest not yet copied.
    for (index, shard) in fixture.shards.iter().enumerate() {
        let name = obcs::shard_name(0, index).expect("a derived shard name");
        std::fs::write(card.join(name.as_str()), shard).expect("copy a shard");
    }

    let mut device = SimCard::new(card.clone());
    assert!(device.next_set_id() > 0, "an id with shards on it is spoken for, listed or not");

    let sent = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the upload runs");
    assert_ne!(sent.device_set_id, 0, "the upload took a fresh id");
    for (index, shard) in fixture.shards.iter().enumerate() {
        let name = obcs::shard_name(0, index).expect("a derived shard name");
        assert_eq!(std::fs::read(card.join(name.as_str())).expect("the rider's shard").len(), shard.len());
    }
    // Finish the copy by hand and it is a map, beside the uploaded one.
    write_set(&card, 0, &fixture);
    assert!(device.scan_set(0).is_some(), "the rider's set survived and mounts");
    assert!(device.scan_set(sent.device_set_id).is_some());

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// The free-space guard, which is necessarily **per file**: the device is not told the set's total
/// until the manifest, and by §5.4 that is last. A shard that would fill the card is refused at the
/// announce with nothing written — the same backstop a single map gets, for the same reason.
#[test]
fn a_card_without_room_refuses_the_shard_at_the_announce() {
    let host = tempdir("full-host");
    let card = tempdir("full-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    // Room for the shard itself, but not for the headroom a ride log and its sidecars need.
    device.free_bytes = Some(plan.files[0].len + MAP_FREE_HEADROOM / 2);

    let first = &plan.files[0];
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    assert_eq!(
        device.send_object(&first.descriptor(), &mut bytes).expect("the link is fine").status,
        TransferStatus::StorageFull
    );
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0, "not one byte, and no token");
    assert!(device.session.is_none());

    // With room, the same set goes up untouched.
    device.free_bytes = Some(plan.total_bytes() + MAP_FREE_HEADROOM * 2);
    let sent = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the set transfers");
    assert!(device.scan_set(sent.device_set_id).is_some());

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// A shard offered **after** the set's manifest committed starts a **new** set. It is the only safe
/// reading: the committed manifest names exactly the files it names, and appending to that id would
/// make it a lie. The new set is an ordinary staged one — invisible, and reclaimed like any other.
#[test]
fn a_shard_after_a_committed_manifest_starts_a_new_set() {
    let host = tempdir("after-host");
    let card = tempdir("after-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    let sent = send(&mut device, &plan, Options::default(), &mut no_progress).expect("the set transfers");
    let committed = sent.device_set_id;

    let first = &plan.files[0];
    let mut bytes = std::fs::File::open(&first.path).expect("open shard 0");
    assert_eq!(
        device.send_object(&first.descriptor(), &mut bytes).expect("the link is fine").status,
        TransferStatus::Committed
    );
    let fresh = device.session.as_ref().expect("a new set is open").id();
    assert_ne!(fresh, committed, "it is a new set, not an append to the committed one");
    assert!(device.scan_set(committed).is_some(), "and the committed set is exactly as it was");
    assert_eq!(device.magic_of(obcs::manifest_name(fresh).unwrap().as_str()), Some([0, 0, 0, 0]));

    device.cable_gone();
    assert!(device.scan_set(committed).is_some(), "the teardown takes the staged set and only it");
    assert!(!device.exists(obcs::manifest_name(fresh).unwrap().as_str()));

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// The abort point with the most to lose: **every shard has landed and the manifest has not been
/// sent**. Nothing on the card is a map, and the whole set — gigabytes of perfectly good shards —
/// must still go, because there is no way to resume it and no surface that could explain it.
#[test]
fn a_set_killed_between_the_last_shard_and_the_manifest_is_reclaimed_whole() {
    let host = tempdir("lastgap-host");
    let card = tempdir("lastgap-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    device.drop_before = Some(plan.files.len()); // the manifest is the last call
    let _ = send(&mut device, &plan, Options::default(), &mut no_progress).expect_err("the link dies");

    let id = device.session.as_ref().expect("the session is still open").id();
    assert!(device.session.as_ref().is_some_and(|s| s.is_complete()), "every shard did land");
    assert!(device.scan_set(id).is_none(), "and none of it is a map");

    // A power cut here is the boot sweep's; a pulled cable is the data plane's. Both take it whole.
    let power_cut = SimCard::new(card.clone());
    assert_eq!(power_cut.sweep(), fixture.shards.len() + 1);
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0);

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// …and the point in between them: the power goes **inside** a shard's stream. Nothing runs
/// afterwards — no abort, no result, no delete — so the card is exactly what the next boot finds,
/// and the boot has to be enough on its own.
#[test]
fn a_power_cut_mid_shard_is_reclaimed_by_the_next_boot() {
    let host = tempdir("midcut-host");
    let card = tempdir("midcut-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    let mut device = SimCard::new(card.clone());
    device.interrupt = Some((3, Interrupt::PowerCut(plan.files[2].len / 3)));
    let err = send(&mut device, &plan, Options::default(), &mut no_progress).expect_err("the device stopped");
    assert!(matches!(err, obc_usb_host::set_transfer::SendError::Link(_)));

    let power_cut = SimCard::new(card.clone());
    assert!(std::fs::read_dir(&card).unwrap().count() > 0, "the corpse is there before the sweep");
    assert!(power_cut.sweep() > 0);
    assert_eq!(std::fs::read_dir(&card).unwrap().count(), 0, "including the half-written shard");

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}

/// Spec §4.1 rule 4, host side: a shard's result echoes the **part** it was sent. A device that
/// answers with a different one has not said "*this* file committed", so the send stops rather than
/// going on to write a manifest over a set the two sides disagree about.
#[test]
fn a_result_that_echoes_the_wrong_part_stops_the_send() {
    let host = tempdir("correlate-host");
    let card = tempdir("correlate-card");
    let (_, fixture) = pair();
    write_set(&host, 1, &fixture);
    let plan = planned(&host, 1);

    /// A device that commits everything correctly but answers one shard with its neighbour's part.
    struct Miscorrelating {
        inner: SimCard,
        at: usize,
        calls: usize,
    }

    impl SetLink for Miscorrelating {
        fn send_object(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> Result<TransferResult, LinkError> {
            self.calls += 1;
            let result = self.inner.send_object(desc, bytes)?;
            if self.calls == self.at {
                let part = SetPart::decode(result.object_id).expect("a shard's result carries a part");
                let wrong = SetPart { shard_count: part.shard_count, index: part.index + 1 };
                return Ok(TransferResult::new(wrong.encode(), result.status, result.committed_offset));
            }
            Ok(result)
        }
    }

    let mut device = Miscorrelating { inner: SimCard::new(card.clone()), at: 2, calls: 0 };
    match send(&mut device, &plan, Options::default(), &mut no_progress) {
        Err(obc_usb_host::set_transfer::SendError::Uncorrelated { filename, expected, echoed }) => {
            assert_eq!(filename, "MS1S01.OBM");
            assert_eq!(expected, SetPart { shard_count: 6, index: 1 }.encode());
            assert_eq!(echoed, SetPart { shard_count: 6, index: 2 }.encode());
        }
        other => panic!("expected a correlation refusal, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&host);
    let _ = std::fs::remove_dir_all(&card);
}
