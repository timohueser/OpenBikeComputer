//! The DFU wire surface (epic #615 S6, #621), host-tested radio-free like the rest of the epic-#267
//! harness: the `fwImage` object type and its transfer through the *unchanged* whole-object machinery,
//! the announce-time size reject, and the `installFw` reply matrix.
//!
//! Storage behaviour (a torn/CRC-failed commit leaves no `/UPDATE.BIN`, a commit overwrites a stale
//! one) is the board crate's thin promote — it reuses the route path's proven, on-glass-gated
//! `copy_with_held_magic`, and this crate has no storage to mock — so here we pin the **protocol-level
//! precondition** those behaviours hang off: a CRC-mismatching stream reports `crcMismatch` (so the
//! board's `fwimage_finish` discards and never promotes), and a matching one reports `committed` (so it
//! promotes exactly once). The file-system assertions are board-built + on-glass-gated.

use obc_ble::descriptor::{ObjectType, Op, TransferControl, TransferStatus};
use obc_ble::{CommandStatus, Crc32, Receiver, CMD_INSTALL_FW};

/// A deterministic pseudo-random payload of `n` bytes (an OBCU-sized object stands in for a real one —
/// the transfer layer is format-blind, §7.6).
fn payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(37).wrapping_add(11)) as u8).collect()
}

/// A `fwImage` upload descriptor for `object` — singleton stage (object id 0), CRC from the
/// production hasher.
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
    // The descriptor carries it verbatim (app → device upload).
    let desc = fwimage_desc(&payload(64));
    let round = TransferControl::decode(&desc.encode()).unwrap();
    assert_eq!(round.ty, ObjectType::FwImage);
    assert_eq!(round, desc);
}

#[test]
fn fwimage_upload_happy_path_commits() {
    // A ~900 KB update stands in as a smaller object — the machinery is size-blind. Deliver it in
    // arbitrary CoC-sized runs; the whole-object CRC verifies once at completion.
    let object = payload(4096);
    let mut rx = Receiver::new(&fwimage_desc(&object)).unwrap();
    for chunk in object.chunks(244) {
        rx.push(chunk);
    }
    assert!(rx.is_complete());
    let result = rx.outcome().expect("complete");
    // Committed ⇒ the board promotes the temp to /UPDATE.BIN exactly once (§7.6).
    assert_eq!(result.status, TransferStatus::Committed);
    assert_eq!(result.committed_offset, object.len() as u32);
    assert_eq!(result.object_id, 0);
}

#[test]
fn fwimage_crc_mismatch_leaves_nothing_to_commit() {
    // A bit-flip in transit: the announced CRC no longer matches the received bytes. The receiver
    // reports `crcMismatch`, so `fwimage_finish` discards the temp and never touches /UPDATE.BIN.
    let mut object = payload(4096);
    let desc = fwimage_desc(&object);
    object[123] ^= 0x01; // corrupt after the CRC was announced
    let mut rx = Receiver::new(&desc).unwrap();
    rx.push(&object);
    let result = rx.outcome().expect("complete");
    assert_eq!(result.status, TransferStatus::CrcMismatch);
    assert_eq!(result.committed_offset, 0, "nothing durable — no UPDATE.BIN is written");
}

#[test]
fn oversize_fwimage_rejected_at_announce() {
    const MAX: u32 = 1_480_000; // stands in for obc_dfu::MAX_IMAGE_LEN (kept out of the wire crate)
                                // At or under the ceiling: accepted (arm the receiver).
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX, MAX), None);
    assert_eq!(TransferStatus::fwimage_announce_reject(1, MAX), None);
    assert_eq!(TransferStatus::fwimage_announce_reject(0, MAX), None);
    // One byte over: rejected at the descriptor write, before any byte streams.
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX + 1, MAX), Some(TransferStatus::Error));
    assert_eq!(TransferStatus::fwimage_announce_reject(u32::MAX, MAX), Some(TransferStatus::Error));
}

#[test]
fn fwimage_announce_ceiling_is_container_sized_not_raw() {
    // The board announces the whole OBCU container as `total_len` (64-byte header + raw image), so it
    // must gate at the *container* ceiling `MAX_IMAGE_LEN + HEADER_LEN`, not the bare raw-image cap.
    // Gating at the raw cap would spuriously reject a raw image in the top 64 bytes of the allowed
    // range that the armer/engine (which gate `image_len` only) would flash fine (DR5, #733).
    const MAX_IMAGE_LEN: u32 = 1_480_000; // obc_dfu::MAX_IMAGE_LEN (kept out of the wire crate)
    const HEADER_LEN: u32 = 64; // obc_dfu::HEADER_LEN
    const MAX_CONTAINER: u32 = MAX_IMAGE_LEN + HEADER_LEN;
    // A raw image exactly at MAX_IMAGE_LEN → container is MAX_IMAGE_LEN + 64: accepted.
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX_IMAGE_LEN + 64, MAX_CONTAINER), None);
    // One raw byte over → container is MAX_IMAGE_LEN + 65: rejected.
    assert_eq!(TransferStatus::fwimage_announce_reject(MAX_IMAGE_LEN + 65, MAX_CONTAINER), Some(TransferStatus::Error));
}

#[test]
fn install_fw_reply_matrix() {
    // ok: staged, not busy, cheaply valid.
    assert_eq!(obc_ble::install_fw_reply(true, false, false), CommandStatus::Ok);
    // noStaged → notFound: nothing on the card.
    assert_eq!(obc_ble::install_fw_reply(false, false, false), CommandStatus::NotFound);
    // busy → busy: recording or an install already pending.
    assert_eq!(obc_ble::install_fw_reply(true, true, false), CommandStatus::Busy);
    // invalid → error: a cheaply-known-bad stage.
    assert_eq!(obc_ble::install_fw_reply(true, false, true), CommandStatus::Error);
}

#[test]
fn install_fw_reply_precedence_is_busy_then_no_staged_then_invalid() {
    // busy wins over everything — the device can't act now regardless of the stage.
    assert_eq!(obc_ble::install_fw_reply(false, true, true), CommandStatus::Busy);
    assert_eq!(obc_ble::install_fw_reply(true, true, true), CommandStatus::Busy);
    // not busy, nothing staged: noStaged wins over a moot invalid flag.
    assert_eq!(obc_ble::install_fw_reply(false, false, true), CommandStatus::NotFound);
}

#[test]
fn install_fw_command_byte_is_three() {
    assert_eq!(CMD_INSTALL_FW, 3);
}
