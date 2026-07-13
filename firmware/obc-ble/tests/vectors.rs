//! `obc-ble`'s **production** codecs against the shared `protocol-vectors/` fixtures — the same
//! files the app's `swift test` pins and `obc-vectors` builds from the spec. `obc-vectors` proves
//! the *bytes* match spec-derived builders; this proves the shipped codecs decode and re-encode
//! those bytes exactly. A drift fails here, there, and on the Swift side.

use obc_ble::descriptor::{ObjectType, Op, StatusMessage, TransferStatus};
use obc_ble::{Config, StoreChanged, TransferControl, TransferResult, VersionRead};
use obc_ble::{Crc32, StatusMessage as Msg};

fn fixture(name: &str) -> Vec<u8> {
    let path = obc_vectors::dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("fixture {name} unreadable ({e}) — run `cargo test -p obc-vectors regenerate -- --ignored`")
    })
}

/// The production CRC agrees with `obc-vectors`' independent spec reference on a real object, and
/// both agree with the descriptor's announced CRC — one number tying the three implementations.
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
        // Re-encoding the decoded descriptor reproduces the fixture byte-for-byte.
        assert_eq!(&desc.encode()[..], &bytes[..], "{name} re-encode");
    }
}

/// The widened `protocolVersion` read (spec §1): `version u16 · store_epoch u32`. The production
/// codec decodes the fixture, sees protocol version 2, and re-encodes it byte-for-byte.
#[test]
fn version_read_vector() {
    let bytes = fixture("version-read.bin");
    assert_eq!(bytes.len(), VersionRead::ENCODED_LEN);
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION, "the fixture pins protocol version 2");
    assert_eq!(vr.store_epoch, 0xA1B2_C3D4);
    assert_eq!(&vr.encode()[..], &bytes[..], "re-encode");
}

/// The version-only `protocolVersion` read (spec §1, card-resident epoch #776): a device with no
/// mounted store serves just the 2-byte version. It carries the protocol version, but the full
/// [`VersionRead`] decode **rejects** it as truncated — exactly the app's "short read ⇒ storeEpoch
/// nil ⇒ ack fail-closed" gate. The 6-byte shape is unchanged when a store is mounted.
#[test]
fn version_read_nostore_vector() {
    let bytes = fixture("version-read-nostore.bin");
    assert_eq!(bytes.len(), 2, "version-only: just the u16 version, no epoch");
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), obc_ble::PROTOCOL_VERSION, "pins protocol version 2");
    assert!(VersionRead::decode(&bytes).is_err(), "a short read is not a full VersionRead — the app fail-closes");
}

/// The download announce (status `msg = 4`): the `msg` byte + the 12-byte descriptor. The
/// production codec decodes it back through the shared `StatusMessage` envelope and the descriptor
/// carries the download's size + CRC (matching the waypoint route).
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

    // Re-encode reproduces the fixture byte-for-byte.
    let (buf, len) = Msg::DownloadAnnounce(desc).encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// A *known* discriminator with a short body is a decode **error** (`Truncated`), while an unknown
/// discriminator stays `Ok(None)` (ignored) — the forward-compat rule cuts exactly between the two.
/// Pinned for `msg = 4`: every truncation of the announce frame is rejected, never misread.
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

    // Re-encode reproduces the fixture.
    let rebuilt = Msg::TransferResult(TransferResult::new(7, TransferStatus::Committed, route_len));
    let (buf, len) = rebuilt.encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// The storage-full reject fixture: a new-route upload (id `0xFFFF`) refused at descriptor-open
/// time because the catalog is full. `status = 6` (`StorageFull`), nothing committed. Pins the
/// discriminant byte so the Swift half decodes the same value.
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

    // Re-encode reproduces the fixture byte-for-byte.
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

/// The `ackRides` command fixture (spec §4.4, cmd 2) decodes through the production codec, its
/// answer fixture decodes as the documented `commandResult` (detail = newly-flagged count), and
/// re-encoding both reproduces the files byte-for-byte — the Swift side pins the same bytes.
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

    // The answer: commandResult{cmd 2, ok, detail 3} — detail is the newly-flagged count.
    let result_bytes = fixture("status-command-result-ack.bin");
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&result_bytes).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_ACK_RIDES, CommandStatus::Ok, 3));
    let (buf, len) = Msg::CommandResult(CommandResult::with_detail(CMD_ACK_RIDES, CommandStatus::Ok, 3)).encode();
    assert_eq!(&buf[..len], &result_bytes[..]);
}

/// `forgetBond` (§4.4 cmd 4, #756): the command is a bare id (no args), and its answer is a plain
/// `commandResult{cmd 4, ok}`. Pins the command byte and the round-trip through the production
/// `commandResult` codec — the Swift side writes `Data([4])` and reads the same envelope back.
#[test]
fn command_forget_bond_round_trip() {
    use obc_ble::{CommandResult, CommandStatus, CMD_FORGET_BOND};

    assert_eq!(CMD_FORGET_BOND, 4, "the wire command id is pinned by the spec (§4.4)");

    // The answer the firmware sends before it clears the bond + drops the link: commandResult(ok),
    // no detail. Round-trips byte-for-byte through the shared codec.
    let (buf, len) = Msg::CommandResult(CommandResult::new(CMD_FORGET_BOND, CommandStatus::Ok)).encode();
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_FORGET_BOND, CommandStatus::Ok, 0));
}

/// `ackRides` decode edges: a `count` promising more ids than the write carries is truncated; a
/// wrong command byte is refused; an empty ack and ignored trailing bytes are both fine.
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

/// Every `TransferStatus` variant round-trips through `as_u8`/`from_u8` and survives a full
/// `transferResult` encode→decode. Pins the discriminant values (`StorageFull == 6`) so a rename
/// or reorder can't silently shift the wire byte.
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

/// An unknown `status` discriminator inside a well-formed `transferResult` envelope is an error
/// (the discriminator IS known — only the status byte is out of range). Forward-compat for the
/// status field lives on the *decoding* side (the app treats a decode failure / unknown status as
/// a generic error); the codec itself rejects a byte it can't name.
#[test]
fn unknown_transfer_status_byte_is_rejected() {
    let (mut buf, len) = Msg::TransferResult(TransferResult::new(0xFFFF, TransferStatus::Committed, 0)).encode();
    buf[3] = 0x7F; // a status code past the highest defined variant
    assert!(StatusMessage::decode(&buf[..len]).is_err());
}

/// The descriptor-open reject rule (issue #452), as a truth table. This is the exact classifier the
/// board crate's `ObjectStore::upload_open` calls; the board crate can't host-test (bare-metal, no
/// `test` crate), so the rule is pinned here.
#[test]
fn upload_open_reject_rule() {
    use obc_ble::TransferControl;
    let new = TransferControl::NEW_OBJECT_ID; // 0xFFFF
    let known = 7u16; // a route the device holds
    let unknown = 42u16; // a named id the device does NOT hold

    // Not full: new + replace both proceed; a named-but-unknown id is a genuine client error.
    assert_eq!(TransferStatus::upload_open_reject(new, false, false), None, "new, room → arm");
    assert_eq!(TransferStatus::upload_open_reject(known, true, false), None, "replace, room → arm");
    assert_eq!(
        TransferStatus::upload_open_reject(unknown, false, false),
        Some(TransferStatus::NotFound),
        "named-but-unknown id, room → notFound"
    );

    // Full: a new upload is rejected up front; a replace-by-id of an existing route is EXEMPT.
    assert_eq!(
        TransferStatus::upload_open_reject(new, false, true),
        Some(TransferStatus::StorageFull),
        "new + full → storageFull"
    );
    assert_eq!(TransferStatus::upload_open_reject(known, true, true), None, "replace at the cap still commits");
    // At the cap, even a named-but-unknown id reads as storage-full (it would grow the catalog).
    assert_eq!(
        TransferStatus::upload_open_reject(unknown, false, true),
        Some(TransferStatus::StorageFull),
        "unknown id + full → storageFull"
    );

    // The rule is object-type-agnostic (epic #526 TR4): the board's `upload_open_trip` passes the
    // *trip* catalog's `catalog_full`/`id_known` into the exact same classifier — a new trip past the
    // 16-trip cap → storageFull before any byte streams, a replace-by-id of a stored trip is exempt.
    assert_eq!(
        TransferStatus::upload_open_reject(new, false, true),
        Some(TransferStatus::StorageFull),
        "new trip + full trip catalog → storageFull"
    );
    assert_eq!(TransferStatus::upload_open_reject(known, true, true), None, "replace trip at the cap still commits");
}

/// An unknown `status` discriminator decodes to `None` (ignored), never an error — forward
/// compatibility.
#[test]
fn unknown_status_discriminator_is_ignored() {
    assert_eq!(StatusMessage::decode(&[0xEE, 0, 0, 0]), Ok(None));
}

/// The `tripList` fixture (spec §7.4) decodes through the production list codec: a 6-byte v2 header
/// (entry_len 76) + one 76-byte entry whose totals sum the trip's two **resolvable** stages
/// (2×2207 m / 2×76 m) while `stage_count` counts all three stored stages (the third is dangling),
/// and whose trailing `crc32` is the trip file's whole-object CRC-32. Re-encoding reproduces the
/// file byte-for-byte — the Swift `TripCodecTests` pin the same bytes.
#[test]
fn trip_list_vector() {
    use obc_ble::{ListHeader, TripListEntry};

    let trip = fixture("trip-v1.bin");
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

/// The trip object type + tripList type decode from the wire `type` byte (spec §4.1: trip = 9,
/// tripList = 10). Pins the discriminants so a reorder can't silently shift the wire byte, and that
/// a download-request descriptor for the tripList round-trips.
#[test]
fn trip_object_types_and_descriptor() {
    assert_eq!(ObjectType::from_u8(9).unwrap(), ObjectType::Trip);
    assert_eq!(ObjectType::from_u8(10).unwrap(), ObjectType::TripList);
    assert_eq!(ObjectType::Trip.as_u8(), 9);
    assert_eq!(ObjectType::TripList.as_u8(), 10);

    // A tripList download request (op=2, type=10, id 0) round-trips through the production codec.
    let desc = TransferControl { op: Op::Download, ty: ObjectType::TripList, object_id: 0, total_len: 0, crc32: 0 };
    assert_eq!(TransferControl::decode(&desc.encode()).unwrap(), desc);
}

/// The `routeList` fixture decodes through the production list codec, its entries agree with the
/// stored route fixtures they describe, and re-encoding reproduces the file byte-for-byte.
#[test]
fn route_list_vector() {
    use obc_ble::{ListHeader, RouteListEntry};

    let route_wp = fixture("route-waypoints.obcr");
    let route_plain = fixture("route-plain.obcr");
    let bytes = fixture("route-list.bin");
    let (h, entry_len) = ListHeader::decode(&bytes).unwrap();
    assert_eq!(h.count, 2);
    assert_eq!(h.total, 2, "nothing truncated");
    assert!(!h.is_truncated());
    assert_eq!(entry_len, RouteListEntry::ENTRY_LEN, "v2 routeList entry is 76 bytes");
    assert_eq!(bytes.len(), ListHeader::object_len(h.count as usize, entry_len));

    let mut rebuilt = ListHeader { count: h.count, total: h.total }.encode(entry_len as u8).to_vec();
    for (k, (byte_len, waypoints, crc)) in
        [(route_wp.len(), 2u16, Crc32::checksum(&route_wp)), (route_plain.len(), 0, Crc32::checksum(&route_plain))]
            .iter()
            .enumerate()
    {
        let off = ListHeader::ENCODED_LEN + k * entry_len;
        let e = RouteListEntry::decode(&bytes[off..off + entry_len]).unwrap();
        assert_eq!(e.byte_len as usize, *byte_len, "entry {k} sizes its stored file");
        assert_eq!(e.waypoint_count, *waypoints);
        assert_eq!(e.name, b"Vector Loop");
        assert_eq!(e.crc32, *crc, "entry {k} carries its content CRC-32");
        assert_eq!((e.distance_m, e.ascent_m, e.point_count), (2207, 76, 9), "OBCR header stats");
        rebuilt.extend_from_slice(&e.encode());
    }
    assert_eq!(rebuilt, bytes, "re-encode");
}
