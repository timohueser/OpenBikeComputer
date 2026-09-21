//! The transfer state machine, exercised end-to-end over an in-memory byte stream.

use obc_ble::descriptor::{ObjectType, Op, TransferControl, TransferStatus};
use obc_ble::transfer::TransferError;
use obc_ble::{Crc32, Receiver, StreamSender};

/// A deterministic pseudo-random payload.
fn payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8).collect()
}

/// The CRC comes from the production hasher.
fn upload_desc(object: &[u8]) -> TransferControl {
    TransferControl {
        op: Op::Upload,
        ty: ObjectType::Echo,
        object_id: 0,
        total_len: object.len() as u32,
        crc32: Crc32::checksum(object),
    }
}

#[test]
fn upload_happy_path_commits() {
    let object = payload(300);
    let mut rx = Receiver::new(&upload_desc(&object)).unwrap();
    assert_eq!(rx.remaining(), 300);

    let consumed = rx.push(&object);
    assert_eq!(consumed, 300);
    assert!(rx.is_complete());
    assert_eq!(rx.committed_offset(), 300);

    let result = rx.outcome().expect("complete");
    assert_eq!(result.status, TransferStatus::Committed);
    assert_eq!(result.committed_offset, 300);
    assert_eq!(result.object_id, 0);
}

#[test]
fn upload_accepts_any_segmentation() {
    let object = payload(257);
    for chunk in [1usize, 2, 7, 64, 244, 256, 300] {
        let mut rx = Receiver::new(&upload_desc(&object)).unwrap();
        for part in object.chunks(chunk) {
            assert_eq!(rx.push(part), part.len());
        }
        assert_eq!(rx.outcome().unwrap().status, TransferStatus::Committed);
    }
}

#[test]
fn crc_corruption_is_rejected_typed() {
    let object = payload(300);
    let mut desc = upload_desc(&object);
    desc.crc32 ^= 0x0000_0001;
    let mut rx = Receiver::new(&desc).unwrap();
    rx.push(&object);

    let result = rx.outcome().unwrap();
    assert_eq!(result.status, TransferStatus::CrcMismatch);
    assert_eq!(result.committed_offset, 0, "nothing durable on a mismatch");
}

#[test]
fn corrupt_payload_same_len_is_rejected() {
    // The right length but a wrong byte: the whole-object CRC catches what the on-air CRC cannot.
    let object = payload(128);
    let desc = upload_desc(&object);
    let mut corrupt = object.clone();
    corrupt[64] ^= 0xFF;
    let mut rx = Receiver::new(&desc).unwrap();
    rx.push(&corrupt);
    assert_eq!(rx.outcome().unwrap().status, TransferStatus::CrcMismatch);
}

#[test]
fn link_checked_receiver_counts_without_software_crc() {
    let object = payload(128);
    let mut desc = upload_desc(&object);
    desc.crc32 ^= 1;
    let mut rx = Receiver::new_link_checked(&desc).unwrap();
    assert_eq!(rx.push(&object[..63]), 63);
    assert_eq!(rx.push(&object[63..]), 65);
    assert_eq!(rx.committed_offset(), 128);
    assert_eq!(rx.outcome().unwrap().status, TransferStatus::Committed);
}

#[test]
fn push_clamps_to_remaining() {
    // The surplus is the caller's protocol error to see.
    let object = payload(50);
    let mut rx = Receiver::new(&upload_desc(&object)).unwrap();
    let mut overrun = object.clone();
    overrun.extend_from_slice(&[0xAA; 10]);
    let consumed = rx.push(&overrun);
    assert_eq!(consumed, 50);
    assert!(rx.is_complete());
    assert_eq!(rx.outcome().unwrap().status, TransferStatus::Committed);
}

#[test]
fn incomplete_has_no_outcome() {
    let object = payload(100);
    let mut rx = Receiver::new(&upload_desc(&object)).unwrap();
    rx.push(&object[..40]);
    assert!(!rx.is_complete());
    assert!(rx.outcome().is_none());
}

#[test]
fn receiver_rejects_wrong_op() {
    let object = payload(100);
    let download = TransferControl { op: Op::Download, ..upload_desc(&object) };
    assert_eq!(Receiver::new(&download).unwrap_err(), TransferError::WrongOp);
}

#[test]
fn echo_loopback_round_trips() {
    // Modeled as push, then echo the consumed bytes back, then compare.
    let object = payload(1024);
    let mut rx = Receiver::new(&upload_desc(&object)).unwrap();
    let mut echoed = Vec::with_capacity(object.len());
    for part in object.chunks(244) {
        let consumed = rx.push(part);
        echoed.extend_from_slice(&part[..consumed]);
    }
    assert_eq!(echoed, object, "byte-identical loopback");
    assert_eq!(rx.outcome().unwrap().status, TransferStatus::Committed);
}

fn download_request(ty: ObjectType) -> TransferControl {
    TransferControl { op: Op::Download, ty, object_id: 0, total_len: 0, crc32: 0 }
}

#[test]
fn download_announces_and_streams() {
    // The read closure stands in for the board's storage read.
    let object = payload(500);
    let crc = Crc32::checksum(&object);
    let read = |at: usize, n: usize| &object[at..at + n];
    let mut tx = StreamSender::new(&download_request(ObjectType::RideList), object.len() as u32, crc).unwrap();

    let announce = tx.announce();
    assert_eq!(announce.op, Op::Download);
    assert_eq!(announce.total_len, 500);
    assert_eq!(announce.crc32, crc);

    let mut sent = Vec::new();
    loop {
        let n = tx.next_chunk_len(244);
        if n == 0 {
            break;
        }
        let chunk = read(tx.position() as usize, n);
        assert!(chunk.len() <= 244);
        sent.extend_from_slice(chunk);
        tx.advance(n);
    }
    assert_eq!(sent, object);
    assert_eq!(tx.outcome().unwrap().status, TransferStatus::Committed);
    assert_eq!(tx.outcome().unwrap().committed_offset, 500);
}

#[test]
fn download_rejects_wrong_op() {
    let upload = TransferControl { op: Op::Upload, ..download_request(ObjectType::Route) };
    assert_eq!(StreamSender::new(&upload, 100, 0).unwrap_err(), TransferError::WrongOp);
}
