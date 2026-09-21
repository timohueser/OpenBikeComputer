//! The production codecs against the shared `specs/vectors/` fixtures, the same files the app's
//! `swift test` pins. `obc-vectors` proves the bytes match spec-derived builders; this proves the
//! shipped codecs decode and re-encode those bytes exactly.

use obc_ble::descriptor::{ObjectType, Op, StatusMessage, TransferStatus};
use obc_ble::{Config, StoreChanged, TransferControl, TransferResult, VersionRead};
use obc_ble::{Crc32, StatusMessage as Msg};

fn fixture(name: &str) -> Vec<u8> {
    let path = obc_vectors::dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("fixture {name} unreadable ({e}) — run `cargo run -p obc-vectors --example regenerate --locked`")
    })
}

/// One number ties the production CRC, the `obc-vectors` reference, and the descriptor's announce.
#[test]
fn production_crc_matches_reference_and_descriptor() {
    let route = fixture("route-waypoints.obcr");
    assert_eq!(Crc32::checksum(&route), obc_vectors::crc32(&route));

    let start = TransferControl::decode(&fixture("transfer-upload-start.bin")).unwrap();
    assert_eq!(start.crc32, Crc32::checksum(&route));
    assert_eq!(start.total_len as usize, route.len());
}

#[test]
fn transfer_control_vectors_round_trip() {
    for (name, op, ty, id) in [
        ("transfer-upload-start.bin", Op::Upload, ObjectType::Route, 0xFFFF),
        ("transfer-download-request.bin", Op::Download, ObjectType::RideList, 0),
        ("transfer-abort.bin", Op::Abort, ObjectType::Route, 0xFFFF),
    ] {
        let bytes = fixture(name);
        assert_eq!(bytes.len(), TransferControl::ENCODED_LEN, "{name} is a 12-byte v2 descriptor");
        let desc = TransferControl::decode(&bytes).unwrap();
        assert_eq!(desc.op, op, "{name} op");
        assert_eq!(desc.ty, ty, "{name} type");
        assert_eq!(desc.object_id, id, "{name} id");
        assert_eq!(&desc.encode()[..], &bytes[..], "{name} re-encode");
    }
}

#[test]
fn version_read_vector() {
    let bytes = fixture("version-read.bin");
    assert_eq!(bytes.len(), VersionRead::ENCODED_LEN);
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION, "the fixture pins protocol version 2");
    assert_eq!(vr.store_epoch, 0xA1B2_C3D4);
    assert_eq!(
        vr.obcm_version,
        Some(obc_formats::obcm::VERSION),
        "the fixture is what a current device serves — the map version its reader reads, not a literal"
    );

    let (enc, len) = vr.encode();
    assert_eq!(&enc[..len], &bytes[..], "re-encode");
}

/// A read without `obcm_version`: it decodes with `obcm_version = None` and re-encodes to the same
/// 6 bytes. `None`, not `Some(0)`, which would read as OBCM v0 and refuse every real map.
#[test]
fn version_read_noobcm_vector() {
    let bytes = fixture("version-read-noobcm.bin");
    assert_eq!(bytes.len(), VersionRead::ENCODED_LEN_NO_OBCM);
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION);
    assert_eq!(vr.store_epoch, 0xA1B2_C3D4, "the epoch is present — this read is not a failed one");
    assert_eq!(vr.obcm_version, None, "an absent trailing field is unknown, never a fabricated default");
    let (enc, len) = vr.encode();
    assert_eq!(&enc[..len], &bytes[..], "re-encode stays 6 bytes — the encoder does not invent the byte either");
}

/// A device with no mounted store serves only the 2-byte version. A full [`VersionRead`] decode
/// rejects that as truncated, which is the app's fail-closed ack gate.
#[test]
fn version_read_nostore_vector() {
    let bytes = fixture("version-read-nostore.bin");
    assert_eq!(bytes.len(), 2, "version-only: just the u16 version, no epoch");
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), obc_ble::PROTOCOL_VERSION, "pins protocol version 2");
    assert!(VersionRead::decode(&bytes).is_err(), "a short read is not a full VersionRead — the app fail-closes");
}

/// The append-only rule: a decoder takes the fields it knows and ignores the bytes past them.
#[test]
fn version_read_ignores_unknown_trailing_bytes() {
    let mut bytes = fixture("version-read.bin");
    bytes.extend_from_slice(&[0xEE, 0xEE, 0xEE]);
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION);
    assert_eq!(vr.store_epoch, 0xA1B2_C3D4);
    assert_eq!(vr.obcm_version, Some(obc_formats::obcm::VERSION));
}

/// The download announce (`msg = 4`): the `msg` byte and the 12-byte descriptor.
#[test]
fn download_announce_vector() {
    let route = fixture("route-waypoints.obcr");
    let bytes = fixture("status-download-announce.bin");
    assert_eq!(bytes.len(), StatusMessage::MAX_ENCODED_LEN, "13 bytes: msg + 12-byte descriptor");

    let StatusMessage::DownloadAnnounce(desc) = StatusMessage::decode(&bytes).unwrap().expect("known msg") else {
        panic!("expected downloadAnnounce")
    };
    assert_eq!(desc.op, Op::Download);
    assert_eq!(desc.ty, ObjectType::Route);
    assert_eq!(desc.object_id, 7);
    assert_eq!(desc.total_len as usize, route.len());
    assert_eq!(desc.crc32, Crc32::checksum(&route));

    let (buf, len) = Msg::DownloadAnnounce(desc).encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// A known discriminator with a short body is a decode error, while an unknown discriminator stays
/// `Ok(None)`. The forward-compatibility rule cuts exactly between the two.
#[test]
fn truncated_download_announce_is_rejected() {
    use obc_ble::DescriptorError;

    let full = fixture("status-download-announce.bin");
    assert_eq!(full.len(), 13);
    for cut in 1..full.len() {
        assert_eq!(
            StatusMessage::decode(&full[..cut]),
            Err(DescriptorError::Truncated),
            "a {cut}-byte prefix of the announce must be Truncated, not ignored or misdecoded"
        );
    }
}

#[test]
fn status_transfer_result_vector() {
    let bytes = fixture("status-transfer-result.bin");
    let route_len = fixture("route-waypoints.obcr").len() as u32;

    let msg = StatusMessage::decode(&bytes).unwrap().expect("known discriminator");
    let StatusMessage::TransferResult(result) = msg else { panic!("expected transferResult") };
    assert_eq!(result.object_id, 7, "assigned id");
    assert_eq!(result.status, TransferStatus::Committed);
    assert_eq!(result.committed_offset, route_len);

    let rebuilt = Msg::TransferResult(TransferResult::new(7, TransferStatus::Committed, route_len));
    let (buf, len) = rebuilt.encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// A new-route upload refused at descriptor-open time because the catalog is full. Pins the
/// `storageFull` discriminant so the Swift half decodes the same value.
#[test]
fn status_transfer_storage_full_vector() {
    let bytes = fixture("status-transfer-storage-full.bin");

    // The status byte lives at offset 3 of the transferResult envelope.
    assert_eq!(bytes[3], 6, "storageFull discriminant is 6");

    let msg = StatusMessage::decode(&bytes).unwrap().expect("known discriminator");
    let StatusMessage::TransferResult(result) = msg else { panic!("expected transferResult") };
    assert_eq!(result.object_id, 0xFFFF, "the rejected new-id request");
    assert_eq!(result.status, TransferStatus::StorageFull);
    assert_eq!(result.committed_offset, 0, "nothing committed");

    let rebuilt = Msg::TransferResult(TransferResult::new(0xFFFF, TransferStatus::StorageFull, 0));
    let (buf, len) = rebuilt.encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

#[test]
fn status_store_changed_vector() {
    let bytes = fixture("status-store-changed.bin");
    let StatusMessage::StoreChanged(s) = StatusMessage::decode(&bytes).unwrap().unwrap() else {
        panic!("expected storeChanged")
    };
    assert_eq!(s.ty, ObjectType::Route);
    assert_eq!(s.revision, 42);

    let (buf, len) = Msg::StoreChanged(StoreChanged { ty: ObjectType::Route, revision: 42 }).encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// The `ackRides` command and its `commandResult` answer, both round-tripped through the
/// production codec.
#[test]
fn command_ack_rides_vector() {
    use obc_ble::{AckRides, CommandResult, CommandStatus, CMD_ACK_RIDES};

    let bytes = fixture("command-ack-rides.bin");
    let ack = AckRides::decode(&bytes).expect("valid ackRides");
    assert_eq!(ack.count(), 3);
    assert_eq!(ack.iter().collect::<Vec<_>>(), [3, 5, 9]);

    let mut out = [0u8; AckRides::encoded_len(3)];
    let len = AckRides::encode(&[3, 5, 9], &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..], "re-encode");

    // The answer: commandResult{cmd 2, ok, detail 3}; detail is the newly-flagged count.
    let result_bytes = fixture("status-command-result-ack.bin");
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&result_bytes).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_ACK_RIDES, CommandStatus::Ok, 3));
    let (buf, len) = Msg::CommandResult(CommandResult::with_detail(CMD_ACK_RIDES, CommandStatus::Ok, 3)).encode();
    assert_eq!(&buf[..len], &result_bytes[..]);
}

/// `forgetBond` is a bare command byte, and its answer is a plain `commandResult{cmd 4, ok}`.
#[test]
fn command_forget_bond_round_trip() {
    use obc_ble::{CommandResult, CommandStatus, CMD_FORGET_BOND};

    assert_eq!(CMD_FORGET_BOND, 4, "the wire command id is pinned by the spec (§4.4)");

    // The firmware sends this answer before it clears the bond and drops the link.
    let (buf, len) = Msg::CommandResult(CommandResult::new(CMD_FORGET_BOND, CommandStatus::Ok)).encode();
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_FORGET_BOND, CommandStatus::Ok, 0));
}

/// The `setClock` fixture decodes as `(utc, offset_min)` and re-encodes to the same 7 bytes. Its
/// answer is a bare `commandResult(ok)`: the clock is not an object, so no `storeChanged` follows.
#[test]
fn command_set_clock_vector() {
    use obc_ble::{CommandResult, CommandStatus, SetClock, CMD_SET_CLOCK};

    assert_eq!(CMD_SET_CLOCK, 5, "the wire command id is pinned by the spec (§4.4, next-free after forgetBond)");

    let bytes = fixture("command-set-clock.bin");
    assert_eq!(bytes.len(), SetClock::ENCODED_LEN, "setClock is a fixed 7-byte write");
    let sc = SetClock::decode(&bytes).expect("valid setClock");
    assert_eq!(sc.utc, 1_783_598_400, "2026-07-09T12:00:00Z");
    assert_eq!(sc.offset_min, 120, "+02:00");

    let mut out = [0u8; SetClock::ENCODED_LEN];
    let len = SetClock::encode(sc.utc, sc.offset_min, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..], "re-encode");

    // The device's answer: commandResult{cmd 5, ok}, with no detail.
    let (buf, len) = Msg::CommandResult(CommandResult::new(CMD_SET_CLOCK, CommandStatus::Ok)).encode();
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_SET_CLOCK, CommandStatus::Ok, 0));
}

/// `setClock` rejects every write a bad phone clock or a wrong length would produce. The gates live
/// in the shared codec, so the firmware and the iOS mirror agree on what is valid.
#[test]
fn set_clock_decode_edges() {
    use obc_ble::{SetClock, SET_CLOCK_MAX_OFFSET_MIN, SET_CLOCK_MIN_UTC};

    let valid = |utc: u32, off: i16| {
        let mut b = [0u8; 7];
        SetClock::encode(utc, off, &mut b).unwrap();
        b
    };

    // setClock has no variable tail, so the write is exactly 7 bytes.
    assert!(SetClock::decode(&[5, 0, 0, 0, 0, 0]).is_err(), "6 bytes: short");
    assert!(SetClock::decode(&[5, 0, 0, 0, 0, 0, 0, 0]).is_err(), "8 bytes: trailing is malformed");
    assert!(SetClock::decode(&valid_cmd(4, SET_CLOCK_MIN_UTC, 0)).is_err(), "cmd 4 is not setClock");
    // A pre-2020 UTC is a bogus phone clock.
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC - 1, 0)).is_err(), "utc before 2020-01-01");
    assert!(SetClock::decode(&valid(0, 0)).is_err(), "utc = 0");
    // Offsets beyond ±14 h are rejected; the bounds themselves pass.
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, SET_CLOCK_MAX_OFFSET_MIN + 1)).is_err(), "offset > +840");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, -SET_CLOCK_MAX_OFFSET_MIN - 1)).is_err(), "offset < −840");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, SET_CLOCK_MAX_OFFSET_MIN)).is_ok(), "+840 is in range");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, -SET_CLOCK_MAX_OFFSET_MIN)).is_ok(), "−840 is in range");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, 0)).is_ok(), "the 2020 epoch itself is accepted");
}

/// Build a 7-byte setClock frame with an arbitrary leading command byte.
fn valid_cmd(cmd: u8, utc: u32, offset_min: i16) -> [u8; 7] {
    let mut b = [0u8; 7];
    b[0] = cmd;
    b[1..5].copy_from_slice(&utc.to_le_bytes());
    b[5..7].copy_from_slice(&offset_min.to_le_bytes());
    b
}
#[test]
fn ack_rides_decode_edges() {
    use obc_ble::{AckRides, DescriptorError};

    assert!(matches!(AckRides::decode(&[2, 3, 1, 0]), Err(DescriptorError::Truncated)), "short of its count");
    assert!(matches!(AckRides::decode(&[2]), Err(DescriptorError::Truncated)), "no count byte");
    assert!(matches!(AckRides::decode(&[1, 0]), Err(DescriptorError::UnknownOp(1))), "not ackRides");
    assert_eq!(AckRides::decode(&[2, 0]).unwrap().count(), 0, "empty ack is well-formed");
    let ack = AckRides::decode(&[2, 1, 7, 0, 0xEE]).unwrap();
    assert_eq!(ack.iter().collect::<Vec<_>>(), [7], "trailing bytes past count are ignored");
}

#[test]
fn config_vector() {
    let bytes = fixture("config-v1.bin");
    let config = Config::decode(&bytes).expect("valid config");
    assert_eq!(config.name, b"OBC Tourer");
    assert_eq!(config.units, 0);

    let mut out = [0u8; Config::MAX_ENCODED];
    let len = Config::encode(&config, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..]);
}

/// Pins the discriminant values, so a rename or a reorder cannot shift the wire byte.
#[test]
fn transfer_status_round_trips_all_variants() {
    use TransferStatus::*;
    for (status, code) in
        [(Committed, 0u8), (CrcMismatch, 1), (Aborted, 2), (Error, 3), (NotFound, 4), (Busy, 5), (StorageFull, 6)]
    {
        assert_eq!(status.as_u8(), code, "{status:?} discriminant");
        assert_eq!(TransferStatus::from_u8(code).unwrap(), status, "{status:?} from_u8");

        let (buf, len) = Msg::TransferResult(TransferResult::new(0xFFFF, status, 0)).encode();
        let StatusMessage::TransferResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
            panic!("expected transferResult")
        };
        assert_eq!(r.status, status, "{status:?} survives encode→decode");
    }
}

/// An unknown status byte inside a well-formed `transferResult` is an error: the discriminator is
/// known, only the status byte is out of range, and the codec rejects a byte it cannot name.
#[test]
fn unknown_transfer_status_byte_is_rejected() {
    let (mut buf, len) = Msg::TransferResult(TransferResult::new(0xFFFF, TransferStatus::Committed, 0)).encode();
    buf[3] = 0x7F; // a status code past the highest defined variant
    assert!(StatusMessage::decode(&buf[..len]).is_err());
}

/// The descriptor-open reject rule as a truth table. The board crate cannot host-test, so the
/// classifier its `ObjectStore::upload_open` calls is pinned here.
#[test]
fn upload_open_reject_rule() {
    use obc_ble::TransferControl;
    let new = TransferControl::NEW_OBJECT_ID;
    let known = 7u16; // a route the device holds
    let unknown = 42u16; // a named id the device does not hold

    // Not full: new and replace both proceed; a named-but-unknown id is a client error.
    assert_eq!(TransferStatus::upload_open_reject(new, false, false), None, "new, room → arm");
    assert_eq!(TransferStatus::upload_open_reject(known, true, false), None, "replace, room → arm");
    assert_eq!(
        TransferStatus::upload_open_reject(unknown, false, false),
        Some(TransferStatus::NotFound),
        "named-but-unknown id, room → notFound"
    );

    // Full: a new upload is rejected up front; a replace by id is exempt.
    assert_eq!(
        TransferStatus::upload_open_reject(new, false, true),
        Some(TransferStatus::StorageFull),
        "new + full → storageFull"
    );
    assert_eq!(TransferStatus::upload_open_reject(known, true, true), None, "replace at the cap still commits");
    // At the cap, a named-but-unknown id reads as storage-full: it would grow the catalog.
    assert_eq!(
        TransferStatus::upload_open_reject(unknown, false, true),
        Some(TransferStatus::StorageFull),
        "unknown id + full → storageFull"
    );

    // The rule is type-agnostic: the board passes the trip catalog's flags to the same classifier.
    assert_eq!(
        TransferStatus::upload_open_reject(new, false, true),
        Some(TransferStatus::StorageFull),
        "new trip + full trip catalog → storageFull"
    );
    assert_eq!(TransferStatus::upload_open_reject(known, true, true), None, "replace trip at the cap still commits");
}

/// The map announce-time reject rule as a truth table, pinned here for the same reason as
/// `upload_open_reject_rule`: the board crate's own tests never run in CI.
#[test]
fn map_announce_reject_rule() {
    use obc_ble::TransferControl;
    const HEADER: u32 = 40; // obc_formats::obcm::HEADER_LEN; the board passes it in
    const HEADROOM: u64 = 8 << 20;
    let new = TransferControl::NEW_OBJECT_ID;
    let map = |id, len, free| TransferStatus::map_announce_reject(id, len, HEADER, free, HEADROOM);

    // A new map that fits, on a card whose free count is readable.
    assert_eq!(map(new, 300_000_000, Some(600 << 20)), None, "new map with room → arm");

    // New-only: the device never rewrites a stored map in place, so no named id is a target. In
    // `upload_open_reject`, by contrast, a known id is the exempt case.
    assert_eq!(map(0, 1_000, Some(u64::MAX)), Some(TransferStatus::NotFound), "id 0 → notFound");
    assert_eq!(map(7, 1_000, Some(u64::MAX)), Some(TransferStatus::NotFound), "a named id → notFound");
    assert_eq!(map(0xFF00, 1_000, Some(u64::MAX)), Some(TransferStatus::NotFound), "even a session-band id → notFound");

    // Too short to be an OBCM: rejected before the free-space arithmetic.
    assert_eq!(map(new, 0, Some(u64::MAX)), Some(TransferStatus::Error), "an empty map → error");
    assert_eq!(map(new, HEADER - 1, Some(u64::MAX)), Some(TransferStatus::Error), "shorter than a header → error");
    assert_eq!(map(new, HEADER, Some(u64::MAX)), None, "exactly a header is structurally acceptable");

    // The free-space guard, including the reserve that keeps a map from taking the last cluster.
    let len = 100u32 << 20;
    assert_eq!(map(new, len, Some(len as u64 + HEADROOM)), None, "exactly len + headroom free → arm");
    assert_eq!(
        map(new, len, Some(len as u64 + HEADROOM - 1)),
        Some(TransferStatus::StorageFull),
        "one byte short of the reserve → storageFull"
    );
    assert_eq!(map(new, len, Some(len as u64)), Some(TransferStatus::StorageFull), "fits but eats the reserve");
    assert_eq!(map(new, len, Some(0)), Some(TransferStatus::StorageFull), "a full card → storageFull");

    // An unmeasurable free count must not become a blanket refusal.
    assert_eq!(map(new, u32::MAX, None), None, "unknown free space → arm (fail late, not never)");
}

/// The first four payload bytes are withheld from the write and replayed at commit, whatever the
/// host's segmentation.
#[test]
fn held_magic_withholds_the_first_four_bytes() {
    use obc_ble::{HeldMagic, MAGIC_LEN};

    // One fat chunk: the magic is held, the rest is written.
    let mut h = HeldMagic::new();
    assert_eq!(h.feed(b"OBCM\x0a\x01\x02\x03"), b"\x0a\x01\x02\x03", "the tail of the first chunk is written");
    assert_eq!(h.take(), Some(*b"OBCM"));
    assert_eq!(h.feed(b"more"), b"more", "later chunks pass straight through");

    // Byte-at-a-time: the same magic, nothing written until it is complete.
    let mut h = HeldMagic::new();
    for (i, byte) in b"OBCM".iter().enumerate() {
        assert_eq!(h.feed(&[*byte]), b"", "byte {i} is held, not written");
        assert_eq!(h.take().is_some(), i + 1 == MAGIC_LEN, "complete only on the {MAGIC_LEN}th byte");
    }
    assert_eq!(h.take(), Some(*b"OBCM"));
    assert_eq!(h.feed(b"body"), b"body");

    // A split across an awkward boundary, and the total written length is always `len - MAGIC_LEN`.
    let payload = b"OBCMxxxxxxxxxxxxxxxxxxxx";
    for split in 0..payload.len() {
        let mut h = HeldMagic::new();
        let a = h.feed(&payload[..split]).len();
        let b = h.feed(&payload[split..]).len();
        assert_eq!(a + b, payload.len() - MAGIC_LEN, "split at {split}: exactly the magic is withheld");
        assert_eq!(h.take(), Some(*b"OBCM"), "split at {split}: the magic still reassembles");
    }

    // An object shorter than a magic never yields one; the announce guard rejects those first.
    let mut h = HeldMagic::new();
    assert_eq!(h.feed(b"OB"), b"");
    assert_eq!(h.take(), None, "a 2-byte object has no magic to replay");
}

#[test]
fn unknown_status_discriminator_is_ignored() {
    assert_eq!(StatusMessage::decode(&[0xEE, 0, 0, 0]), Ok(None));
}

/// The `tripList` fixture: a 6-byte header and one 76-byte entry whose totals sum the trip's two
/// resolvable stages, while `stage_count` counts all three stored stages.
#[test]
fn trip_list_vector() {
    use obc_ble::{ListHeader, TripListEntry};

    let trip = fixture("trip-v2.bin");
    let bytes = fixture("trip-list.bin");
    let (h, entry_len) = ListHeader::decode(&bytes).unwrap();
    assert_eq!(h.count, 1);
    assert_eq!(h.total, 1, "nothing truncated");
    assert!(!h.is_truncated());
    assert_eq!(entry_len, TripListEntry::ENTRY_LEN, "v2 tripList entry is 76 bytes");
    assert_eq!(bytes.len(), ListHeader::object_len(h.count as usize, entry_len));

    let e = TripListEntry::decode(&bytes[ListHeader::ENCODED_LEN..ListHeader::ENCODED_LEN + entry_len]).unwrap();
    assert_eq!(e.object_id, 1, "the trip's own device id (separate counter, §4.1)");
    assert_eq!(e.byte_len as usize, trip.len(), "byte_len sizes the stored trip file");
    assert_eq!((e.total_distance_m, e.total_ascent_m), (2 * 2207, 2 * 76), "summed over resolvable stages");
    assert_eq!(e.stage_count, 3, "counts every stored stage, dangling ref included");
    assert_eq!(e.name, b"Alpen Traverse");
    assert_eq!(e.crc32, Crc32::checksum(&trip), "trailing crc32 = the trip file's whole-object CRC");

    let mut rebuilt = ListHeader { count: h.count, total: h.total }.encode(entry_len as u8).to_vec();
    rebuilt.extend_from_slice(&e.encode());
    assert_eq!(rebuilt, bytes, "re-encode");
}

/// Pins trip = 9 and tripList = 10, so a reorder cannot shift the wire byte.
#[test]
fn trip_object_types_and_descriptor() {
    assert_eq!(ObjectType::from_u8(9).unwrap(), ObjectType::Trip);
    assert_eq!(ObjectType::from_u8(10).unwrap(), ObjectType::TripList);
    assert_eq!(ObjectType::Trip.as_u8(), 9);
    assert_eq!(ObjectType::TripList.as_u8(), 10);

    let desc = TransferControl { op: Op::Download, ty: ObjectType::TripList, object_id: 0, total_len: 0, crc32: 0 };
    assert_eq!(TransferControl::decode(&desc.encode()).unwrap(), desc);
}

/// Pins the `map` type byte at 16, and that the 11-15 reserved band keeps rejecting.
#[test]
fn map_object_type_and_reserved_band() {
    assert_eq!(ObjectType::from_u8(16).unwrap(), ObjectType::Map);
    assert_eq!(ObjectType::Map.as_u8(), 16);
    for reserved in 11..=15 {
        assert!(ObjectType::from_u8(reserved).is_err(), "type {reserved} is reserved (sensors, M4)");
    }

    let desc = TransferControl {
        op: Op::Upload,
        ty: ObjectType::Map,
        object_id: 0,
        total_len: 300_000_000,
        crc32: 0x1234_5678,
    };
    assert_eq!(TransferControl::decode(&desc.encode()).unwrap(), desc);
}

/// The object-type band the map upload lives in. The values 17-19 named the files of a multi-file
/// map; they are not re-issued, in the same way a retired GATT UUID is not.
#[test]
fn map_object_types() {
    assert_eq!(ObjectType::from_u8(16).unwrap(), ObjectType::Map);
    assert_eq!(ObjectType::Map.as_u8(), 16);
    for retired in 17..=19u8 {
        assert!(ObjectType::from_u8(retired).is_err(), "{retired} was a volume-set type and is not re-issued");
    }

    assert!(ObjectType::from_u8(21).is_err(), "21 is not a type yet");
    for reserved in 11..=15u8 {
        assert!(ObjectType::from_u8(reserved).is_err(), "{reserved} stays reserved for the sensor work");
    }

    assert!(ObjectType::Map.is_map_payload(), "a map streams into its final file with the magic held back");
    for ty in [ObjectType::Route, ObjectType::Trip, ObjectType::FwImage, ObjectType::Echo] {
        assert!(!ty.is_map_payload(), "{ty:?} stages through UPLOAD.TMP");
    }
}

#[test]
fn route_list_vector() {
    use obc_ble::{ListHeader, RouteListEntry};

    let route_wp = fixture("route-waypoints.obcr");
    let route_plain = fixture("route-plain.obcr");
    let bytes = fixture("route-list.bin");
    let (h, entry_len) = ListHeader::decode(&bytes).unwrap();
    assert_eq!(h.count, 3);
    assert_eq!(h.total, 3, "nothing truncated");
    assert!(!h.is_truncated());
    assert_eq!(entry_len, RouteListEntry::ENTRY_LEN, "v2 routeList entry is 84 bytes (76 core + expiry tail)");
    assert_eq!(entry_len, 76);
    assert_eq!(bytes.len(), ListHeader::object_len(h.count as usize, entry_len));

    let plain_crc = Crc32::checksum(&route_plain);
    let expect = [
        (7u16, route_wp.len(), 2u16, Crc32::checksum(&route_wp)),
        (8, route_plain.len(), 0, plain_crc),
        (9, route_plain.len(), 0, plain_crc),
    ];
    let mut rebuilt = ListHeader { count: h.count, total: h.total }.encode(entry_len as u8).to_vec();
    for (k, &(id, byte_len, waypoints, crc)) in expect.iter().enumerate() {
        let off = ListHeader::ENCODED_LEN + k * entry_len;
        let e = RouteListEntry::decode(&bytes[off..off + entry_len]).unwrap();
        assert_eq!(e.object_id, id, "entry {k} id");
        assert_eq!(e.byte_len as usize, byte_len, "entry {k} sizes its stored file");
        assert_eq!(e.waypoint_count, waypoints);
        assert_eq!(e.name, b"Vector Loop");
        assert_eq!(e.crc32, crc, "entry {k} carries its content CRC-32");
        assert_eq!((e.distance_m, e.ascent_m, e.point_count), (2207, 76, 9), "OBCR header stats");
        rebuilt.extend_from_slice(&e.encode());
    }
    assert_eq!(rebuilt, bytes, "re-encode");
}
