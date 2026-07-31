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

/// The device's receive path over a directory instead of a FAT volume: the announce policy, the
/// held-back magic, the session, the commit checks, the abort, and the boot sweep — composed in the
/// order `link::transfer::classify_transfer` → `usb::data_plane::run_upload` → `ObjectStore` →
/// `sd::Storage` composes them on the board.
struct SimCard {
    root: PathBuf,
    session: Option<obc_app::SetUpload>,
    max_shards: u8,
    /// Drop the link before the *n*-th `send_object` — the mid-set kill.
    drop_before: Option<usize>,
    calls: usize,
}

impl SimCard {
    fn new(root: PathBuf) -> Self {
        SimCard { root, session: None, max_shards: DEVICE_MAX_SHARDS, drop_before: None, calls: 0 }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// `Storage::next_set_id_from_scan`: one past the highest **listed** set, i.e. one that
    /// validates whole. A torn set's manifest has no magic, so it is invisible and its id is
    /// re-derived — the same property that stops the map path leaking an id per interruption.
    fn next_set_id(&self) -> u16 {
        let mut next = 0u16;
        for entry in std::fs::read_dir(&self.root).expect("read the card root").flatten() {
            let name = entry.file_name().to_string_lossy().to_uppercase();
            let Some(id) = obcs::parse_manifest_name(name.as_bytes()) else { continue };
            if self.scan_set(id).is_some() {
                next = next.max(id + 1);
            }
        }
        next
    }

    /// `Storage::set_identity` + `set_shard_totals`: parse the manifest and check every shard it
    /// names is present at the recorded size, version and bbox. `None` = *this is not a map*.
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
        }
        Some(manifest)
    }

    /// `Storage::set_upload_begin`: clear anything under this id (§5.4's "delete the old manifest
    /// **before** overwriting any of its shards"), then write the zero-magic in-flight token.
    fn begin_set(&self, id: u16) {
        self.delete_set(id);
        let name = obcs::manifest_name(id).expect("a derived manifest name");
        std::fs::write(self.path(name.as_str()), [0u8; 4]).expect("write the set token");
    }

    /// `Storage::delete_set`: `obcs::delete_plan`'s ordered list — manifest first, then every shard
    /// name to the cap — with the same stop-if-the-manifest-survives rule.
    fn delete_set(&self, id: u16) -> usize {
        let Some(plan) = obcs::delete_plan(id) else { return 0 };
        let mut removed = 0;
        for (step, derived) in plan.iter().enumerate() {
            let path = self.path(derived.as_str());
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(_) if step == 0 && path.exists() => return 0,
                Err(_) => {}
            }
        }
        removed
    }

    /// `ObjectStore::set_upload_abort` — what the data plane runs on a dropped link or an `op=3`.
    fn link_reset(&mut self) {
        if let Some(session) = self.session.take() {
            self.delete_set(session.id());
        }
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
            if obc_app::sweep_verdict(self.magic_of(name.as_str())) == obc_app::SweepVerdict::Reclaim {
                swept += self.delete_set(*id);
            }
        }
        for (name, id) in &shards {
            if manifests.contains(id) {
                continue;
            }
            if obc_app::orphan_shard_verdict(self.magic_of(name)) == obc_app::SweepVerdict::Reclaim
                && std::fs::remove_file(self.path(name)).is_ok()
            {
                swept += 1;
            }
        }
        swept
    }

    fn magic_of(&self, name: &str) -> Option<[u8; 4]> {
        let bytes = std::fs::read(self.path(name)).ok()?;
        bytes.get(..4).map(|m| [m[0], m[1], m[2], m[3]])
    }

    /// Stream one object into `name` with its leading four magic bytes withheld, exactly as
    /// `usb::data_plane::run_upload` does: the `Receiver`'s CRC sees every payload byte, only the
    /// *write* skips the held prefix. Returns the held magic once the whole object has arrived.
    fn stream(&self, name: &str, desc: &TransferControl, bytes: &mut dyn Read) -> (Receiver, HeldMagic) {
        let mut rx = Receiver::new(desc).expect("an upload descriptor");
        let mut held = HeldMagic::new();
        let mut file = std::fs::File::create(self.path(name)).expect("create the target");
        file.write_all(&[0u8; 4]).expect("the placeholder magic");
        let mut buf = [0u8; 4096];
        while !rx.is_complete() {
            let n = bytes.read(&mut buf).expect("read the source");
            if n == 0 {
                break;
            }
            let consumed = rx.push(&buf[..n]);
            let write = held.feed(&buf[..consumed]);
            file.write_all(write).expect("append");
        }
        (rx, held)
    }

    /// Patch a committed file's real magic over the placeholder — the commit point.
    fn patch_magic(&self, name: &str, magic: [u8; 4]) {
        use std::io::Seek;
        let mut file = std::fs::OpenOptions::new().write(true).open(self.path(name)).expect("reopen to commit");
        file.seek(std::io::SeekFrom::Start(0)).expect("seek");
        file.write_all(&magic).expect("the commit write");
    }
}

impl SetLink for SimCard {
    fn send_object(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> Result<TransferResult, LinkError> {
        self.calls += 1;
        if self.drop_before == Some(self.calls) {
            return Err(LinkError("the cable was pulled".into()));
        }
        match desc.ty {
            ObjectType::MapShard => Ok(self.recv_shard(desc, bytes)),
            ObjectType::MapSet => Ok(self.recv_manifest(desc, bytes)),
            other => panic!("a set transfer offered {other:?}"),
        }
    }
}

impl SimCard {
    /// `ObjectStore::set_shard_open` → `set_shard_begin` → `set_shard_finish`.
    fn recv_shard(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> TransferResult {
        let Some(part) = SetPart::decode(desc.object_id) else {
            return TransferResult::new(desc.object_id, TransferStatus::NotFound, 0);
        };
        let fresh = match obc_app::shard_announce(self.session.as_ref(), part.shard_count, part.index, self.max_shards)
        {
            Ok(fresh) => fresh,
            Err(reject) => return TransferResult::new(desc.object_id, reject_status(reject), 0),
        };
        if desc.total_len < obc_formats::obcm::HEADER_LEN as u32 {
            return TransferResult::new(desc.object_id, TransferStatus::Error, 0);
        }
        if fresh {
            let id = self.next_set_id();
            self.begin_set(id);
            self.session = Some(obc_app::SetUpload::new(id, part.shard_count));
        }
        let id = self.session.as_ref().expect("a session is open").id();
        let name = obcs::shard_name(id, part.index as usize).expect("a derived shard name").as_str().to_string();

        let (rx, held) = self.stream(&name, desc, bytes);
        let outcome = match rx.outcome() {
            Some(o) if o.status == TransferStatus::Committed => o,
            other => {
                // A failed shard drops itself and leaves the session standing — the host re-sends
                // one file, not the set.
                let _ = std::fs::remove_file(self.path(&name));
                return TransferResult::new(desc.object_id, other.map_or(TransferStatus::Error, |o| o.status), 0);
            }
        };
        let magic = held.take().expect("an OBCM-sized object carries a magic");
        // `Storage::set_shard_commit`: the header must validate with the real magic spliced in,
        // before it is written.
        let mut header = std::fs::read(self.path(&name)).expect("read back");
        header[0..4].copy_from_slice(&magic);
        if obc_formats::obcm::validate_header_prefix(&header).is_err() {
            let _ = std::fs::remove_file(self.path(&name));
            return TransferResult::new(desc.object_id, TransferStatus::Error, 0);
        }
        self.patch_magic(&name, magic);
        if let Some(session) = &mut self.session {
            session.mark(part.index);
        }
        TransferResult::new(part.encode(), TransferStatus::Committed, outcome.committed_offset)
    }

    /// `ObjectStore::set_manifest_open` → `set_manifest_begin` → `set_manifest_finish`, i.e. the
    /// place §5.4's manifest-last rule is *enforced*.
    fn recv_manifest(&mut self, desc: &TransferControl, bytes: &mut dyn Read) -> TransferResult {
        if desc.object_id != TransferControl::NEW_OBJECT_ID {
            return TransferResult::new(desc.object_id, TransferStatus::NotFound, 0);
        }
        if let Err(reject) = obc_app::manifest_announce(self.session.as_ref(), desc.total_len) {
            return TransferResult::new(desc.object_id, reject_status(reject), 0);
        }
        let id = self.session.as_ref().expect("the announce proved a session").id();
        let name = obcs::manifest_name(id).expect("a derived manifest name").as_str().to_string();

        let (rx, held) = self.stream(&name, desc, bytes);
        self.session = None;
        let outcome = match rx.outcome() {
            Some(o) if o.status == TransferStatus::Committed => o,
            other => {
                self.delete_set(id);
                return TransferResult::new(desc.object_id, other.map_or(TransferStatus::Error, |o| o.status), 0);
            }
        };
        let magic = held.take().expect("a manifest carries a magic");
        // `Storage::validate_committed_manifest`: re-read with the magic spliced in, parse against
        // §5.3, and check it against the shards actually on the card — before the magic is written.
        let mut manifest_bytes = std::fs::read(self.path(&name)).expect("read back");
        manifest_bytes[0..4].copy_from_slice(&magic);
        std::fs::write(self.path(&name), &manifest_bytes).expect("splice");
        if self.scan_set(id).is_none() {
            self.delete_set(id);
            return TransferResult::new(desc.object_id, TransferStatus::Error, 0);
        }
        TransferResult::new(id, TransferStatus::Committed, outcome.committed_offset)
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

    // What the data plane does when it sees the link drop.
    device.link_reset();
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
