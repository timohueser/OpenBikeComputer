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
            assert!(desc.offset > 0 && desc.offset < route_len, "{name} resumes mid-object");
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
