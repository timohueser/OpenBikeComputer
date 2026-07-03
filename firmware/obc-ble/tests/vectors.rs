//! `obc-ble`'s **production** codecs against the shared `protocol-vectors/` fixtures — the same
//! files the app's `swift test` pins and `obc-vectors` builds from the spec. `obc-vectors` proves
//! the *bytes* match spec-derived builders; this proves the codecs the firmware actually ships
//! decode and re-encode those bytes exactly. A drift fails here, there, and on the Swift side.

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

/// An unknown `status` discriminator decodes to `None` (ignored), never an error — forward
/// compatibility (spec §4.3).
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
