//! `obc-ble`'s **production** codecs against the shared `protocol-vectors/` fixtures — the same
//! files the app's `swift test` pins and `obc-vectors` builds from the spec. `obc-vectors` proves
//! the *bytes* match spec-derived builders; this proves the shipped codecs decode and re-encode
//! those bytes exactly. A drift fails here, there, and on the Swift side.

use obc_ble::descriptor::{ObjectType, Op, StatusMessage, TransferStatus};
use obc_ble::{Config, ObjectStoreDigest, StoreChanged, TransferControl, TransferResult};
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
    for (name, op, ty, id, offset_is_mid) in [
        ("transfer-upload-start.bin", Op::Upload, ObjectType::Route, 0xFFFF, false),
        ("transfer-upload-resume.bin", Op::Upload, ObjectType::Route, 0xFFFF, true),
        ("transfer-download-request.bin", Op::Download, ObjectType::RideList, 0, false),
        ("transfer-abort.bin", Op::Abort, ObjectType::Route, 0xFFFF, false),
    ] {
        let bytes = fixture(name);
        let desc = TransferControl::decode(&bytes).unwrap();
        assert_eq!(desc.op, op, "{name} op");
        assert_eq!(desc.ty, ty, "{name} type");
        assert_eq!(desc.object_id, id, "{name} id");
        // Re-encoding the decoded descriptor reproduces the fixture byte-for-byte.
        assert_eq!(&desc.encode()[..], &bytes[..], "{name} re-encode");

        let route_len = fixture("route-waypoints.obcr").len() as u32;
        if offset_is_mid {
            // Shape stability: a non-zero offset still DECODES byte-exactly — the
            // semantic reject (transfers restart, not resume) happens in the
            // transfer layer, not the codec.
            assert!(desc.offset > 0 && desc.offset < route_len, "{name} carries a mid-object offset");
        }
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
fn object_store_vector() {
    let bytes = fixture("object-store.bin");
    let digest = ObjectStoreDigest::decode(&bytes).unwrap();
    assert_eq!(digest, ObjectStoreDigest { revision: 42, route_count: 3, ride_count: 5 });
    assert_eq!(&digest.encode()[..], &bytes[..]);
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
}

/// An unknown `status` discriminator decodes to `None` (ignored), never an error — forward
/// compatibility.
#[test]
fn unknown_status_discriminator_is_ignored() {
    assert_eq!(StatusMessage::decode(&[0xEE, 0, 0, 0]), Ok(None));
}

/// The `routeList` fixture decodes through the production list codec, its entries agree with the
/// stored route fixtures they describe, and re-encoding reproduces the file byte-for-byte.
#[test]
fn route_list_vector() {
    use obc_ble::{ListHeader, RouteListEntry};

    let bytes = fixture("route-list.bin");
    let (h, entry_len) = ListHeader::decode(&bytes).unwrap();
    assert_eq!(h.count, 2);
    assert_eq!(bytes.len(), ListHeader::object_len(h.count as usize));

    let mut rebuilt = ListHeader { count: h.count }.encode().to_vec();
    for (k, (byte_len, waypoints)) in
        [(fixture("route-waypoints.obcr").len(), 2u16), (fixture("route-plain.obcr").len(), 0)].iter().enumerate()
    {
        let off = ListHeader::ENCODED_LEN + k * entry_len;
        let e = RouteListEntry::decode(&bytes[off..off + entry_len]).unwrap();
        assert_eq!(e.byte_len as usize, *byte_len, "entry {k} sizes its stored file");
        assert_eq!(e.waypoint_count, *waypoints);
        assert_eq!(e.name, b"Vector Loop");
        assert_eq!((e.distance_m, e.ascent_m, e.point_count), (2207, 76, 9), "OBCR header stats");
        rebuilt.extend_from_slice(&e.encode());
    }
    assert_eq!(rebuilt, bytes, "re-encode");
}
