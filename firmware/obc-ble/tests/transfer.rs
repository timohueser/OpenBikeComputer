//! The transfer state machine, exercised end-to-end over an in-memory byte stream — the
//! host-verified half of the A5 data plane, before any of it touches the radio. Covers the happy
//! path, resume from every offset (property-style), CRC-corruption rejection, arbitrary CoC
//! segmentation, over-run, and the echo loopback the board wires on glass.

use obc_ble::descriptor::{ObjectType, Op, TransferControl, TransferStatus};
use obc_ble::transfer::TransferError;
use obc_ble::{Crc32, Receiver, Sender};

/// A deterministic pseudo-random payload of `n` bytes (a route-sized object).
fn payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8).collect()
}

/// A fresh upload descriptor for `object` (echo id 0, offset 0), CRC from the production hasher.
fn upload_desc(object: &[u8]) -> TransferControl {
    TransferControl {
        op: Op::Upload,
        ty: ObjectType::Echo,
        object_id: 0,
        total_len: object.len() as u32,
        crc32: Crc32::checksum(object),
        offset: 0,
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
    // The CoC delivers arbitrary byte runs (spec §5): every chunk split must reach the same commit.
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
fn resume_from_every_offset_commits() {
    // Property-style: drop after k bytes, resume from k with the committed-prefix CRC, finish — the
    // whole-object CRC must still verify at every split (spec §4.2 "keeps its running CRC state").
    let object = payload(200);
    let whole = upload_desc(&object);
    for k in 0..=object.len() {
        let mut first = Receiver::new(&whole).unwrap();
        assert_eq!(first.push(&object[..k]), k);
        assert_eq!(first.committed_offset(), k as u32);

        // The resume descriptor differs only in offset; seed the CRC from the committed prefix.
        let resume_desc = TransferControl { offset: k as u32, ..whole };
        let mut second = Receiver::resumed(&resume_desc, first.crc()).unwrap();
        assert_eq!(second.remaining(), (object.len() - k) as u32);
        second.push(&object[k..]);

        let result = second.outcome().unwrap_or_else(|| panic!("incomplete at k={k}"));
        assert_eq!(result.status, TransferStatus::Committed, "resume at {k}");
        assert_eq!(result.committed_offset, object.len() as u32);
    }
}

#[test]
fn crc_corruption_is_rejected_typed() {
    let object = payload(300);
    let mut desc = upload_desc(&object);
    desc.crc32 ^= 0x0000_0001; // flip one bit of the announced CRC
    let mut rx = Receiver::new(&desc).unwrap();
    rx.push(&object);

    let result = rx.outcome().unwrap();
    assert_eq!(result.status, TransferStatus::CrcMismatch);
    assert_eq!(result.committed_offset, 0, "nothing durable on a mismatch");
}

#[test]
fn corrupt_payload_same_len_is_rejected() {
    // The link delivered the right length but a wrong byte — the whole-object CRC is exactly what
    // catches this (the on-air CRC can't; spec §6).
    let object = payload(128);
    let desc = upload_desc(&object);
    let mut corrupt = object.clone();
    corrupt[64] ^= 0xFF;
    let mut rx = Receiver::new(&desc).unwrap();
    rx.push(&corrupt);
    assert_eq!(rx.outcome().unwrap().status, TransferStatus::CrcMismatch);
}

#[test]
fn push_clamps_to_remaining() {
    // A receiver never consumes past total_len; the surplus is the caller's protocol error to see.
    let object = payload(50);
    let mut rx = Receiver::new(&upload_desc(&object)).unwrap();
    let mut overrun = object.clone();
    overrun.extend_from_slice(&[0xAA; 10]); // 10 bytes too many
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
fn receiver_rejects_wrong_op_and_bad_offset() {
    let object = payload(100);
    let download = TransferControl { op: Op::Download, ..upload_desc(&object) };
    assert_eq!(Receiver::new(&download).unwrap_err(), TransferError::WrongOp);

    let bad_offset = TransferControl { offset: 101, ..upload_desc(&object) };
    assert_eq!(Receiver::new(&bad_offset).unwrap_err(), TransferError::OffsetPastTotal);
}

#[test]
fn echo_loopback_round_trips() {
    // The A5 loopback: the device receives an echo object and streams back exactly what it received,
    // CRC-verified — modeled here as Receiver.push → echo the consumed bytes → compare.
    let object = payload(1024);
    let mut rx = Receiver::new(&upload_desc(&object)).unwrap();
    let mut echoed = Vec::with_capacity(object.len());
    for part in object.chunks(244) {
        let consumed = rx.push(part);
        echoed.extend_from_slice(&part[..consumed]); // what the board writes back over the CoC
    }
    assert_eq!(echoed, object, "byte-identical loopback");
    assert_eq!(rx.outcome().unwrap().status, TransferStatus::Committed);
}

// ---- Sender (download direction) ----

fn download_request(ty: ObjectType, offset: u32) -> TransferControl {
    TransferControl { op: Op::Download, ty, object_id: 0, total_len: 0, crc32: 0, offset }
}

#[test]
fn download_announces_and_streams() {
    let object = payload(500);
    let mut tx = Sender::new(&download_request(ObjectType::RideList, 0), &object).unwrap();

    let announce = tx.announce();
    assert_eq!(announce.op, Op::Download);
    assert_eq!(announce.total_len, 500);
    assert_eq!(announce.crc32, Crc32::checksum(&object));
    assert_eq!(announce.offset, 0);

    let mut sent = Vec::new();
    while let Some(chunk) = tx.next_chunk(244) {
        assert!(chunk.len() <= 244);
        sent.extend_from_slice(chunk);
    }
    assert_eq!(sent, object);
    assert_eq!(tx.outcome().unwrap().status, TransferStatus::Committed);
    assert_eq!(tx.outcome().unwrap().committed_offset, 500);
}

#[test]
fn download_resume_streams_the_tail() {
    let object = payload(500);
    let offset = 200;
    let mut tx = Sender::new(&download_request(ObjectType::Ride, offset), &object).unwrap();
    assert_eq!(tx.announce().offset, offset);
    assert_eq!(tx.announce().total_len, 500); // CRC still covers the whole object

    let mut sent = Vec::new();
    while let Some(chunk) = tx.next_chunk(244) {
        sent.extend_from_slice(chunk);
    }
    assert_eq!(sent, &object[offset as usize..]);
    assert!(tx.is_complete());
}

#[test]
fn sender_rejects_wrong_op_and_bad_offset() {
    let object = payload(100);
    let upload = TransferControl { op: Op::Upload, ..download_request(ObjectType::Ride, 0) };
    assert_eq!(Sender::new(&upload, &object).unwrap_err(), TransferError::WrongOp);
    let bad = download_request(ObjectType::Ride, 101);
    assert_eq!(Sender::new(&bad, &object).unwrap_err(), TransferError::OffsetPastTotal);
}
