//! The DFU wire surface: the `fwImage` object type through the unchanged whole-object transfer
//! machinery, the announce-time size reject, and the `installFw` reply matrix. This crate has no
//! storage, so these tests pin the protocol precondition the board's promote hangs off: a
//! CRC-mismatching stream reports `crcMismatch`, a matching one reports `committed`.

use obc_ble::descriptor::{ObjectType, Op, TransferControl, TransferStatus};
use obc_ble::{CommandStatus, Crc32, Receiver, CMD_INSTALL_FW};

/// A deterministic pseudo-random payload. The transfer layer is format-blind.
fn payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(37).wrapping_add(11)) as u8).collect()
}

/// The stage is a singleton, so the object id is 0. The CRC comes from the production hasher.
fn fwimage_desc(object: &[u8]) -> TransferControl {
    TransferControl {
        op: Op::Upload,
        ty: ObjectType::FwImage,
        object_id: 0,
        total_len: object.len() as u32,
        crc32: Crc32::checksum(object),
    }
}

#[test]
fn fwimage_object_type_round_trips_at_id_5() {
    assert_eq!(ObjectType::FwImage.as_u8(), 5);
    assert_eq!(ObjectType::from_u8(5), Ok(ObjectType::FwImage));
    let desc = fwimage_desc(&payload(64));
    let round = TransferControl::decode(&desc.encode()).unwrap();
    assert_eq!(round.ty, ObjectType::FwImage);
    assert_eq!(round, desc);
}

#[test]
fn fwimage_upload_happy_path_commits() {
    // A small object stands in for a real update: the machinery is size-blind.
    let object = payload(4096);
    let mut rx = Receiver::new(&fwimage_desc(&object)).unwrap();
    for chunk in object.chunks(244) {
        rx.push(chunk);
    }
    assert!(rx.is_complete());
    let result = rx.outcome().expect("complete");
    // Committed means the board promotes the temp to /UPDATE.BIN exactly once.
    assert_eq!(result.status, TransferStatus::Committed);
    assert_eq!(result.committed_offset, object.len() as u32);
    assert_eq!(result.object_id, 0);
}

#[test]
fn fwimage_crc_mismatch_leaves_nothing_to_commit() {
    // On crcMismatch the board discards the temp and never touches /UPDATE.BIN.
    let mut object = payload(4096);
    let desc = fwimage_desc(&object);
    object[123] ^= 0x01; // corrupt after the CRC is announced
    let mut rx = Receiver::new(&desc).unwrap();
    rx.push(&object);
    let result = rx.outcome().expect("complete");
    assert_eq!(result.status, TransferStatus::CrcMismatch);
    assert_eq!(result.committed_offset, 0, "nothing durable — no UPDATE.BIN is written");
}

#[test]
fn oversize_fwimage_rejected_at_announce() {
    const MAX: u32 = 1_480_000; // stands in for obc_dfu::MAX_IMAGE_LEN
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX, MAX), None);
    assert_eq!(TransferStatus::fwimage_announce_reject(1, MAX), None);
    assert_eq!(TransferStatus::fwimage_announce_reject(0, MAX), None);
    // Over the ceiling: rejected at the descriptor write, before any byte streams.
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX + 1, MAX), Some(TransferStatus::Error));
    assert_eq!(TransferStatus::fwimage_announce_reject(u32::MAX, MAX), Some(TransferStatus::Error));
}

#[test]
fn fwimage_announce_ceiling_is_container_sized_not_raw() {
    // `total_len` is the whole OBCU container, so the ceiling must be container-sized. A raw-image
    // ceiling would reject an image in the top 64 bytes of the range that the armer flashes fine.
    const MAX_IMAGE_LEN: u32 = 1_480_000; // obc_dfu::MAX_IMAGE_LEN
    const HEADER_LEN: u32 = 64; // obc_dfu::HEADER_LEN
    const MAX_CONTAINER: u32 = MAX_IMAGE_LEN + HEADER_LEN;
    // A raw image at the cap: the container is 64 bytes larger.
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX_IMAGE_LEN + 64, MAX_CONTAINER), None);
    // One raw byte over.
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX_IMAGE_LEN + 65, MAX_CONTAINER), Some(TransferStatus::Error));
}

/// The command answers from edge state only: it can act, or it is busy. Whether a package is
/// staged, and whether it is valid, are the on-device flow's own answers a second later.
#[test]
fn install_fw_reply_matrix() {
    assert_eq!(obc_ble::install_fw_reply(false), CommandStatus::Ok);
    // Recording, or an install already pending.
    assert_eq!(obc_ble::install_fw_reply(true), CommandStatus::Busy);
}

#[test]
fn install_fw_command_byte_is_three() {
    assert_eq!(CMD_INSTALL_FW, 3);
}
