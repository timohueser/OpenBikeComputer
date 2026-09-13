//! Exact archive proof uses the real catalog and metadata publication path.
use super::*;
use obc_storage::flat::{metadata, EntryFlags, Mutation, PutSource, Store};

fn receipt(request: u32, id: u64, revision: u64, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::from(flat_harness::STORE.0);
    body.extend(id.to_le_bytes());
    body.extend(revision.to_le_bytes());
    body.extend((bytes.len() as u64).to_le_bytes());
    body.extend(crc32(bytes).to_le_bytes());
    client::frame(9, request, &body)
}

fn rows<D: BlockDevice>(device: &Device<D>) -> Vec<metadata::Row> {
    let mut bytes = [0; metadata::MAX_LEN];
    metadata::Metadata::new(&device.store).load(&device.store, &mut bytes).unwrap().rows().collect()
}

#[test]
fn receipt_survives_lost_reply_reopen_and_preserves_the_first_stamp() {
    let disk = formatted_card(801);
    let mut device = boot(&disk);
    let bytes = b"finalized ride";
    let (id, rev) = device.finish_recording(bytes, "ride");
    device.control(&client::get(1, id, rev));
    assert!(rows(&device).is_empty(), "GET delivery is not archive proof");
    let request = receipt(2, id, rev, bytes);
    let before = device.store.sequence();
    let first = Answer::of(device.control(&request).answer());
    assert!(!first.is_error());
    assert_eq!((first.opcode, first.request), (9, 2));
    assert_eq!(first.u64_at(0), before + 1);
    assert_eq!(&first.body[8..], &[0; 8]);
    assert_eq!(rows(&device)[0].timestamp, 0);
    // Lose the answer, disconnect and mount again: the catalog is the deduplication authority.
    drop(device);
    disk.reboot();
    let mut device = boot(&disk);
    device.link_up(Link::Usb, Ceilings::for_usb(4_112).unwrap());
    let again = Answer::of(device.control_on(Link::Usb, &request).answer());
    assert_eq!(again.body, first.body);
    // An unrelated commit changes sequence, but cannot invalidate exact archive possession.
    device.seed(ObjectKind::Route, b"route", "route");
    let sequence = device.store.sequence();
    let again = Answer::of(device.control(&receipt(3, id, rev, bytes)).answer());
    assert!(!again.is_error());
    assert_eq!(again.u64_at(0), sequence);
    assert_eq!(device.store.sequence(), sequence);
    // The policy integration is separate; seed its eventual nonzero row through the substrate.
    let mut owner = metadata::Metadata::new(&device.store);
    let mut workspace = [0; metadata::MAX_LEN];
    let mut image = owner.load(&device.store, &mut workspace).unwrap();
    let mut row = image.rows().next().unwrap();
    row.timestamp = 12345;
    image.set(row).unwrap();
    owner.replace(&device.store, &mut image, None).unwrap();
    let sequence = device.store.sequence();
    let answer = Answer::of(device.control(&receipt(4, id, rev, bytes)).answer());
    assert!(!answer.is_error());
    assert_eq!(&answer.body[8..12], &12345u32.to_le_bytes());
    assert_eq!(device.store.sequence(), sequence);
}

#[test]
fn every_source_component_and_current_finalized_head_are_required() {
    let disk = formatted_card(802);
    let mut device = boot(&disk);
    let bytes = b"ride";
    let (id, rev) = device.seed(ObjectKind::Ride, bytes, "ride");
    let good = receipt(1, id, rev, bytes);
    for offset in [16, 31, 32, 40, 48, 56] {
        let mut bad = good.clone();
        bad[offset] ^= 0x80;
        let answer = Answer::of(device.control(&bad).answer());
        expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
        assert!(rows(&device).is_empty());
    }
    let (route, revision) = device.seed(ObjectKind::Route, bytes, "route");
    let (recording, recording_rev) = device.seed_recording(1024);
    for (id, rev) in [(route, revision), (recording, recording_rev)] {
        let answer = Answer::of(device.control(&receipt(2, id, rev, bytes)).answer());
        expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
    }
    let old = device.store.entries().find(|entry| entry.id.0 == id).unwrap();
    let mut allocation = device.store.allocate(bytes.len() as u64).unwrap();
    device.store.write(&mut allocation, bytes).unwrap();
    device
        .store
        .commit(&[
            Mutation::Put {
                meta: obc_storage::flat::EntryMeta { flags: EntryFlags::RETAINED, ..old },
                source: PutSource::Amend,
            },
            Mutation::Put {
                meta: obc_storage::flat::EntryMeta { revision: obc_storage::flat::Revision(rev + 1), ..old },
                source: PutSource::Fresh(allocation),
            },
        ])
        .unwrap();
    let answer = Answer::of(device.control(&good).answer());
    expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
    let current = receipt(3, id, rev + 1, bytes);
    assert!(!Answer::of(device.control(&current).answer()).is_error());
    device.control(&client::remove(4, id, rev + 1));
    let sequence = device.store.sequence();
    let absent = Answer::of(device.control(&current).answer());
    expect_error(&absent, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
    assert_eq!(device.store.sequence(), sequence, "source-gone retry cannot recreate proof");
}

#[test]
fn busy_and_unreadable_metadata_never_acknowledge_a_receipt() {
    let disk = formatted_card(803);
    let mut device = boot(&disk);
    let (id, rev) = device.seed(ObjectKind::Ride, b"ride", "ride");
    let receipt = receipt(2, id, rev, b"ride");
    device.control(&client::put(1, 0, 0, b"route", ROUTE, false, "route"));
    let busy = Answer::of(device.control(&receipt).answer());
    expect_error(&busy, ErrorCode::Busy, detail::busy::TRANSFER);
    device.link_lost();
    device.seed(ObjectKind::Metadata, b"not metadata", "");
    let sequence = device.store.sequence();
    let malformed = Answer::of(device.control(&receipt).answer());
    expect_error(&malformed, ErrorCode::MediaIo, 0);
    assert_eq!(device.store.sequence(), sequence);
}

#[test]
fn receipt_failure_retry_and_uncertain_readback_use_the_global_fence() {
    fn run(fault: Option<(MediaOp, u32)>) -> (u32, u32) {
        let disk = formatted_card(804);
        let media = FaultOnce::new(&disk);
        let mut device = boot(&media);
        let (id, rev) = device.seed(ObjectKind::Ride, b"ride", "ride");
        let request = receipt(1, id, rev, b"ride");
        let before = disk.ledger().len();
        if let Some((operation, skip)) = fault {
            media.fault_after(operation, skip);
        }
        let answer = Answer::of(device.control(&request).answer());
        let ledger = disk.ledger();
        let trace = &ledger[before..];
        let syncs = trace.iter().filter(|(_, op, _)| *op == MediaOp::Sync).count() as u32;
        let last_sync = trace.iter().rposition(|(_, op, _)| *op == MediaOp::Sync);
        let reads =
            last_sync.map_or(0, |at| trace[..=at].iter().filter(|(_, op, _)| *op == MediaOp::Read).count() as u32);
        if let Some((operation, _)) = fault {
            assert!(media.fired());
            assert!(answer.is_error());
            if operation == MediaOp::Write {
                assert!(device.store.mode().writable());
                assert!(!Answer::of(device.control(&request).answer()).is_error());
            } else {
                assert_eq!(device.store.mode(), obc_storage::flat::Mode::RemountRequired);
                let again = Answer::of(device.control(&request).answer());
                expect_error(&again, ErrorCode::ReadOnly, detail::read_only::CATALOG_UNREADABLE);
            }
        } else {
            assert!(!answer.is_error());
        }
        drop(device);
        disk.reboot();
        let mut reopened = boot(&disk);
        let answer = Answer::of(reopened.control(&request).answer());
        assert!(!answer.is_error());
        assert_eq!(rows(&reopened)[0].timestamp, 0);
        (syncs, reads)
    }
    let (syncs, reads) = run(None);
    run(Some((MediaOp::Write, 0)));
    run(Some((MediaOp::Sync, syncs - 1)));
    run(Some((MediaOp::Read, reads)));
}
