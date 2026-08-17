//! The protocol-v4 engine against a real flat store: one behaviour per rule of
//! `FLAT_Store_Protocol.md` §3 and §4.
//!
//! The card underneath is the sim-backed [`FlatStore`](obc_storage::flat::FlatStore), so a claim
//! about the catalog here is a claim about the bytes on a card and not about a mock that agreed to
//! be convenient.

mod flat_harness;

use flat_harness::{boot, client, crc32, formatted_card, payload, Answer, Device};
use obc_link::flat::store::Policy;
use obc_link::flat::wire::{detail, ErrorCode};
use obc_link::flat::{ObjectId, ObjectKind, Revision};

const ROUTE: u16 = 1;
const WEATHER: u16 = 4;
const RIDE: u16 = 3;
const UPDATE: u16 = 7;

/// A payload that crosses the harness's 1 KiB stage several times and ends part-way through it.
fn body() -> Vec<u8> {
    payload(2_600)
}

fn error(answer: &Answer) -> (u16, u16, u64) {
    answer.error()
}

fn expect_error(answer: &Answer, code: ErrorCode, detail: u16) {
    assert_eq!((error(answer).0, error(answer).1), (code.value(), detail), "wrong refusal: {answer:?}");
}

/// Announces, streams and completes one upload, and returns the answer.
fn upload(
    device: &mut Device<'_>,
    request: u32,
    id: u64,
    expected: u64,
    bytes: &[u8],
    kind: u16,
    name: &str,
) -> Answer {
    let wire = device.control(&client::put(request, id, expected, bytes, kind, false, name));
    assert!(wire.control.is_empty(), "an admitted PUT answers nothing until the last byte");
    let mut last = None;
    for record in client::stream_all(request, bytes, 1_008) {
        let wire = device.stream(&record);
        if !wire.control.is_empty() {
            last = Some(Answer::of(wire.answer()));
        }
    }
    last.expect("the last stream record is answered")
}

#[test]
fn a_first_list_carries_the_store_identity_and_an_empty_catalog() {
    let disk = formatted_card(1);
    let mut device = boot(&disk);
    let answer = Answer::of(device.control(&client::list(1, None)).answer());
    assert!(!answer.is_error());
    assert!(!answer.has_more());
    assert_eq!(answer.request, 1);
    assert_eq!(&answer.body[0..16], &[0x11; 16], "the page carries the StoreId a client keys its cache on");
    assert_eq!(answer.u64_at(16), 1, "the commit sequence a paged listing is checked against");
    assert_eq!(answer.body.len(), 24, "an empty catalog is a page with no entries");
}

#[test]
fn a_create_streams_commits_and_answers_with_the_assigned_identity() {
    let disk = formatted_card(2);
    let mut device = boot(&disk);
    let bytes = body();
    let answer = upload(&mut device, 0x2A01, 0, 0, &bytes, ROUTE, "Grimsel Loop");

    assert!(!answer.is_error(), "{answer:?}");
    assert_eq!(answer.request, 0x2A01);
    assert_eq!(answer.u64_at(0), 1, "the device assigned the first ObjectId");
    assert_eq!(answer.u64_at(8), 1, "a create publishes Revision 1");
    assert_eq!(answer.u64_at(16), bytes.len() as u64);
    assert_eq!(answer.u32_at(24), crc32(&bytes));

    let entry = device.entry(1).expect("the catalog holds the new object");
    assert_eq!(entry.payload_len, bytes.len() as u64);
    assert_eq!(entry.payload_crc, crc32(&bytes));
    assert_eq!(entry.name.as_bytes(), b"Grimsel Loop");
    assert!(device.is_quiet(), "the engine is idle once the transfer is answered");

    // And the bytes came back exactly, which is the only proof the staging path is honest.
    let wire = device.control(&client::get(2, 1, 0));
    assert_eq!(wire.payload(), bytes);
    let answer = Answer::of(wire.answer());
    assert_eq!(answer.u64_at(0), 1, "the revision served");
    assert_eq!(answer.u64_at(8), bytes.len() as u64);
    assert_eq!(answer.u32_at(16), crc32(&bytes));
}

#[test]
fn a_download_is_paced_by_the_link_ceiling_and_ends_with_the_answer() {
    let disk = formatted_card(3);
    let mut device = boot(&disk);
    let bytes = body();
    device.seed(ObjectKind::Route, &bytes, "Furka");

    let wire = device.control(&client::get(7, 1, 0));
    assert_eq!(wire.stream.len(), 3, "2,600 bytes at a 1,024-byte SDU is three records");
    for (index, record) in wire.stream.iter().enumerate() {
        assert_eq!(u32::from_le_bytes(record[0..4].try_into().unwrap()), 7, "the frame names its own request");
        let offset = u64::from_le_bytes(record[4..12].try_into().unwrap());
        assert_eq!(offset, index as u64 * 1_008, "frames are contiguous and ascending");
    }
    assert_eq!(wire.payload(), bytes);
    assert!(device.is_quiet());
}

#[test]
fn a_second_transfer_is_busy_and_names_the_live_one() {
    let disk = formatted_card(4);
    let mut device = boot(&disk);
    let bytes = body();
    device.control(&client::put(0x11, 0, 0, &bytes, ROUTE, false, "first"));

    let answer = Answer::of(device.control(&client::put(0x22, 0, 0, &bytes, ROUTE, false, "second")).answer());
    assert_eq!(error(&answer), (ErrorCode::Busy.value(), detail::busy::TRANSFER, 0x11));

    let answer = Answer::of(device.control(&client::get(0x33, 1, 0)).answer());
    assert_eq!(error(&answer).0, ErrorCode::Busy.value(), "a GET is the same one transfer at a time");

    // A LIST is not a transfer and is answered while one runs.
    assert!(!Answer::of(device.control(&client::list(0x44, None)).answer()).is_error());
}

#[test]
fn a_replace_is_a_compare_and_swap_on_the_head_revision() {
    let disk = formatted_card(5);
    let mut device = boot(&disk);
    let bytes = body();
    device.seed(ObjectKind::Route, &bytes, "first");

    let answer = Answer::of(device.control(&client::put(1, 1, 7, &bytes, ROUTE, false, "wrong")).answer());
    assert_eq!(error(&answer), (ErrorCode::RevisionConflict.value(), detail::revision_conflict::HEAD_DIFFERS, 1));

    let answer = Answer::of(device.control(&client::put(2, 9, 1, &bytes, ROUTE, false, "absent")).answer());
    expect_error(&answer, ErrorCode::RevisionConflict, detail::revision_conflict::HEAD_ABSENT);

    let answer = upload(&mut device, 3, 1, 1, &bytes, ROUTE, "second");
    assert_eq!(answer.u64_at(8), 2, "the replace published the next revision");
    assert_eq!(device.entries().len(), 1, "an ordinary replace leaves a head and nothing else");
    assert_eq!(device.entry(1).unwrap().name.as_bytes(), b"second");
}

#[test]
fn a_retaining_replace_keeps_the_displaced_revision_and_frees_the_previous_one() {
    let disk = formatted_card(6);
    let mut device = boot(&disk);
    let bytes = body();
    device.seed(ObjectKind::WeatherBundle, &bytes, "one");

    let wire = device.control(&client::put(1, 1, 1, &bytes, WEATHER, true, "two"));
    assert!(wire.control.is_empty());
    for record in client::stream_all(1, &bytes, 1_008) {
        device.stream(&record);
    }
    let entries = device.entries();
    assert_eq!(entries.len(), 2, "the displaced revision is still there");
    assert_eq!(entries[0].revision.0, 1);
    assert!(entries[0].flags.has(obc_storage::flat::EntryFlags::RETAINED));
    assert_eq!(entries[1].revision.0, 2);

    // A second retaining replace frees the first retained revision.
    let bytes2 = payload(1_500);
    let wire = device.control(&client::put(2, 1, 2, &bytes2, WEATHER, true, "three"));
    assert!(wire.control.is_empty());
    for record in client::stream_all(2, &bytes2, 1_008) {
        device.stream(&record);
    }
    let entries = device.entries();
    assert_eq!(entries.len(), 2, "at most one retained revision survives");
    assert_eq!(entries[0].revision.0, 2);
    assert_eq!(entries[1].revision.0, 3);

    // §3.6: retention is legal only for a kind whose reader needs continuity.
    let answer = Answer::of(device.control(&client::put(3, 0, 0, &bytes, ROUTE, true, "route")).answer());
    expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
}

#[test]
fn a_remove_takes_the_retained_revision_with_it() {
    let disk = formatted_card(7);
    let mut device = boot(&disk);
    let bytes = body();
    device.seed(ObjectKind::WeatherBundle, &bytes, "one");
    let wire = device.control(&client::put(1, 1, 1, &bytes, WEATHER, true, "two"));
    assert!(wire.control.is_empty());
    for record in client::stream_all(1, &bytes, 1_008) {
        device.stream(&record);
    }
    assert_eq!(device.entries().len(), 2);
    let free = device.free_extents();

    let answer = Answer::of(device.control(&client::remove(2, 1, 1)).answer());
    expect_error(&answer, ErrorCode::RevisionConflict, detail::revision_conflict::HEAD_DIFFERS);

    let answer = Answer::of(device.control(&client::remove(3, 1, 2)).answer());
    assert!(!answer.is_error(), "{answer:?}");
    assert_eq!(answer.body.len(), 8, "the answer is the new commit sequence");
    assert!(device.entries().is_empty(), "the retained revision went with the head");
    assert_eq!(device.free_extents(), free + 2, "both revisions' extents came back");

    let answer = Answer::of(device.control(&client::remove(4, 1, 2)).answer());
    expect_error(&answer, ErrorCode::NotFound, detail::not_found::OBJECT);
}

#[test]
fn status_is_the_reconcile_path_and_answers_all_three_states() {
    let disk = formatted_card(8);
    let mut device = boot(&disk);
    let bytes = body();
    device.seed(ObjectKind::Route, &bytes, "one");

    let answer = Answer::of(device.control(&client::status(1, 9, 1)).answer());
    assert_eq!(answer.body[0], 0, "absent");
    assert_eq!(answer.u64_at(4), 0);

    let answer = Answer::of(device.control(&client::status(2, 1, 1)).answer());
    assert_eq!(answer.body[0], 1, "committed");
    assert_eq!(answer.u64_at(4), 1);
    assert_eq!(answer.u64_at(12), bytes.len() as u64);
    assert_eq!(answer.u32_at(20), crc32(&bytes));

    let answer = Answer::of(device.control(&client::status(3, 1, 2)).answer());
    assert_eq!(answer.body[0], 2, "superseded: the object exists at a different revision");
    assert_eq!(answer.u64_at(4), 1, "and the answer says which");
}

#[test]
fn a_cancel_answers_both_sides_and_leaves_the_card_untouched() {
    let disk = formatted_card(9);
    let mut device = boot(&disk);
    let bytes = body();
    let free = device.free_extents();

    device.control(&client::put(0x50, 0, 0, &bytes, ROUTE, false, "half"));
    device.stream(&client::stream(0x50, 0, &bytes[..1_008]));
    assert_eq!(device.free_extents(), free - 1, "the allocation is holding an extent");

    let wire = device.control(&client::cancel(0x51, 0x50));
    assert_eq!(wire.control.len(), 2, "the CANCEL and the transfer each get an answer");
    let cancel = Answer::of(&wire.control[0]);
    assert_eq!(cancel.body, [0], "0 cancelled");
    let transfer = Answer::of(&wire.control[1]);
    assert_eq!(transfer.request, 0x50);
    expect_error(&transfer, ErrorCode::Cancelled, detail::cancelled::BY_CLIENT);

    assert_eq!(device.free_extents(), free, "the allocation was released");
    assert!(device.entries().is_empty(), "the catalog never heard of it");
    assert!(device.is_quiet());

    let answer = Answer::of(device.control(&client::cancel(0x52, 0x50)).answer());
    assert_eq!(answer.body, [1], "1 no such transfer");
}

#[test]
fn a_cancelled_download_closes_its_handle() {
    let disk = formatted_card(10);
    let mut device = boot(&disk);
    let bytes = body();
    let (id, revision) = device.seed(ObjectKind::Route, &bytes, "one");

    // One record goes out and the client cancels with the rest of the payload still to come.
    let wire = device.control_upto(&client::get(0x61, id, 0), 1);
    assert_eq!(wire.stream.len(), 1);
    let wire = device.control(&client::cancel(0x62, 0x61));
    assert_eq!(wire.control.len(), 2);
    expect_error(&Answer::of(&wire.control[1]), ErrorCode::Cancelled, detail::cancelled::BY_CLIENT);

    // A hold the engine failed to close would keep the entry's extents out of the allocator when it
    // is removed; one extent coming back is the proof it did close.
    assert_eq!(device.remove_and_measure(id, revision), 1);
}

#[test]
fn a_payload_that_fails_its_crc_commits_nothing() {
    let disk = formatted_card(11);
    let mut device = boot(&disk);
    let bytes = body();
    let free = device.free_extents();
    let announced = client::put(1, 0, 0, &bytes, ROUTE, false, "corrupt");

    device.control(&announced);
    let mut corrupted = bytes.clone();
    corrupted[7] ^= 0xFF;
    let mut answer = None;
    for record in client::stream_all(1, &corrupted, 1_008) {
        let wire = device.stream(&record);
        if !wire.control.is_empty() {
            answer = Some(Answer::of(wire.answer()));
        }
    }
    let answer = answer.expect("the last record is answered");
    assert_eq!(
        error(&answer),
        (ErrorCode::ChecksumFailure.value(), detail::checksum_failure::PAYLOAD, u64::from(crc32(&bytes)))
    );
    assert!(device.entries().is_empty(), "nothing was published");
    assert_eq!(device.free_extents(), free, "and the allocation came back");
}

#[test]
fn a_gap_or_an_overlap_in_the_stream_terminates_the_transfer() {
    for (name, offset) in [("a gap", 2_016u64), ("an overlap", 0)] {
        let disk = formatted_card(12);
        let mut device = boot(&disk);
        let bytes = body();
        let free = device.free_extents();
        device.control(&client::put(1, 0, 0, &bytes, ROUTE, false, "gap"));
        device.stream(&client::stream(1, 0, &bytes[..1_008]));
        let wire = device.stream(&client::stream(1, offset, &bytes[..8]));
        let answer = Answer::of(wire.answer());
        expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::STREAM_OFFSET);
        assert_eq!(device.free_extents(), free, "{name} released the allocation");
        assert!(device.is_quiet());
    }
}

#[test]
fn a_stream_record_for_no_live_transfer_is_discarded_in_silence() {
    let disk = formatted_card(13);
    let mut device = boot(&disk);
    let wire = device.stream(&client::stream(0x99, 0, &[1, 2, 3]));
    assert!(wire.control.is_empty() && wire.stream.is_empty(), "late frames are ordinary in-flight traffic");

    let bytes = body();
    device.control(&client::put(1, 0, 0, &bytes, ROUTE, false, "live"));
    let wire = device.stream(&client::stream(2, 0, &bytes[..16]));
    assert!(wire.control.is_empty(), "a frame naming another request is not this transfer's");
    assert_eq!(device.free_extents(), 63, "and it did not disturb the live one");
}

#[test]
fn the_device_owned_kinds_and_the_flagged_entries_are_refused() {
    let disk = formatted_card(14);
    let mut device = boot(&disk);
    let bytes = body();
    let (ride, _) = device.seed_recording(4 * 1_024 * 1_024);

    for kind in [RIDE, 8] {
        let answer = Answer::of(device.control(&client::put(1, 0, 0, &bytes, kind, false, "no")).answer());
        expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
    }
    let answer = Answer::of(device.control(&client::get(2, ride, 0)).answer());
    expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
    let answer = Answer::of(device.control(&client::remove(3, ride, 1)).answer());
    expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
    let answer = Answer::of(device.control(&client::put(4, ride, 1, &bytes, RIDE, false, "no")).answer());
    expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);

    // A ride is still listed — a client syncs it once RECORDING has cleared.
    let answer = Answer::of(device.control(&client::list(5, Some(RIDE))).answer());
    assert_eq!(answer.body.len(), 24 + 88);
}

#[test]
fn an_object_with_no_bytes_is_a_remove_and_never_a_put() {
    let disk = formatted_card(15);
    let mut device = boot(&disk);
    let answer = Answer::of(device.control(&client::put(1, 0, 0, &[], ROUTE, false, "empty")).answer());
    expect_error(&answer, ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
    assert_eq!(device.free_extents(), 64, "nothing was reserved for it");
}

#[test]
fn a_full_reservation_table_is_busy_and_never_invalid_request() {
    let disk = formatted_card(16);
    let mut device = boot(&disk);
    let bytes = body();
    let first = device.hog(1_024);
    let second = device.hog(1_024);

    let answer = Answer::of(device.control(&client::put(1, 0, 0, &bytes, ROUTE, false, "no room")).answer());
    assert_eq!(error(&answer).0, ErrorCode::Busy.value(), "a full table is transient, not the client's fault");

    device.release(first);
    device.release(second);
    let answer = upload(&mut device, 2, 0, 0, &bytes, ROUTE, "room again");
    assert!(!answer.is_error());
}

#[test]
fn a_card_with_no_flat_store_answers_read_only_to_everything() {
    let disk = flat_harness::blank_card(17);
    let mut device = boot(&disk);
    let bytes = body();
    for record in [
        client::list(1, None),
        client::status(2, 1, 1),
        client::get(3, 1, 0),
        client::put(4, 0, 0, &bytes, ROUTE, false, "no"),
        client::remove(5, 1, 1),
        client::arm(6, 1, 1),
    ] {
        let answer = Answer::of(device.control(&record).answer());
        expect_error(&answer, ErrorCode::ReadOnly, detail::read_only::UNFORMATTED);
    }
}

#[test]
fn a_list_pages_at_the_link_ceiling_and_a_stale_cursor_restarts_the_listing() {
    let disk = formatted_card(18);
    let mut device = boot(&disk);
    let bytes = payload(64);
    for index in 0..5 {
        device.seed(ObjectKind::Route, &bytes, &format!("object {index}"));
    }

    let first = Answer::of(device.control(&client::list(1, None)).answer());
    assert!(first.has_more(), "a 244-byte ceiling carries two of five entries");
    assert_eq!(first.body.len(), 24 + 2 * 88);
    let sequence = first.u64_at(16);
    let last = (
        u64::from_le_bytes(first.body[24 + 88..24 + 96].try_into().unwrap()),
        u64::from_le_bytes(first.body[24 + 96..24 + 104].try_into().unwrap()),
    );
    assert_eq!(last, (2, 1));

    let second = Answer::of(device.control(&client::list_from(2, None, last, sequence)).answer());
    assert!(second.has_more());
    assert_eq!(u64::from_le_bytes(second.body[24..32].try_into().unwrap()), 3, "the page resumes strictly after");

    // A commit moves the sequence on, and the cursor the client holds is stale.
    device.seed(ObjectKind::Route, &bytes, "later");
    let answer = Answer::of(device.control(&client::list_from(3, None, last, sequence)).answer());
    assert_eq!(
        error(&answer),
        (ErrorCode::CatalogChanged.value(), detail::catalog_changed::LISTING, sequence + 1),
        "and the answer carries the sequence to restart from"
    );

    // A first page never fails that way, because it declares no expectation.
    assert!(!Answer::of(device.control(&client::list(4, None)).answer()).is_error());
}

#[test]
fn a_kind_filter_lists_only_that_kind() {
    let disk = formatted_card(19);
    let mut device = boot(&disk);
    device.seed(ObjectKind::Route, &payload(64), "route");
    device.seed(ObjectKind::Trip, &payload(64), "trip");

    let answer = Answer::of(device.control(&client::list(1, Some(2))).answer());
    assert_eq!(answer.body.len(), 24 + 88, "one trip");
    assert_eq!(u64::from_le_bytes(answer.body[24..32].try_into().unwrap()), 2);
    assert_eq!(u16::from_le_bytes(answer.body[24 + 28..24 + 30].try_into().unwrap()), 2, "kind 2");
}

#[test]
fn the_link_going_away_releases_everything_and_answers_nobody() {
    let disk = formatted_card(20);
    let mut device = boot(&disk);
    let bytes = body();
    let free = device.free_extents();

    device.control(&client::put(1, 0, 0, &bytes, ROUTE, false, "half"));
    device.stream(&client::stream(1, 0, &bytes[..1_008]));
    device.link_lost();
    assert!(device.is_quiet(), "no error is owed to a peer that is gone");
    assert_eq!(device.free_extents(), free);
    assert!(device.entries().is_empty());

    // And the client restarts from zero on the next link.
    let answer = upload(&mut device, 2, 0, 0, &bytes, ROUTE, "again");
    assert!(!answer.is_error(), "{answer:?}");
    assert_eq!(answer.u64_at(0), 1);
}

/// A device with an update path: the two hooks §4 needs, recorded.
#[derive(Default)]
struct Armer {
    reserve: u64,
    refuse: Option<u16>,
    handed_off: Option<((ObjectId, Revision), (ObjectId, Revision))>,
}

impl Policy for Armer {
    fn validate_package(&mut self, _package: ObjectId, _revision: Revision) -> Result<u64, u16> {
        match self.refuse {
            Some(reason) => Err(reason),
            None => Ok(self.reserve),
        }
    }

    fn hand_off(&mut self, package: (ObjectId, Revision), reserve: (ObjectId, Revision)) -> Result<(), u16> {
        self.handed_off = Some((package, reserve));
        Ok(())
    }
}

#[test]
fn arming_commits_one_reserve_hands_off_and_reboots() {
    let disk = formatted_card(21);
    let mut device = boot(&disk);
    let package = payload(4_096);
    let (id, revision) = device.seed(ObjectKind::UpdatePackage, &package, "v2");
    let mut armer = Armer { reserve: 900_000, ..Armer::default() };

    let wire = device.control_with(&client::arm(1, id, revision), &mut armer);
    assert!(wire.reboot, "§4 step 5: the answer reaches the transport, then the device reboots");
    let answer = Answer::of(wire.answer());
    assert!(!answer.is_error(), "{answer:?}");
    let reserve = answer.u64_at(0);
    assert_eq!(reserve, 2, "the reserve is the next ObjectId");
    assert_eq!(armer.handed_off, Some(((ObjectId(id), Revision(revision)), (ObjectId(reserve), Revision(1)))));

    let entry = device.entry(reserve).expect("the reserve is in the catalog");
    assert_eq!(entry.kind, obc_storage::flat::ObjectKind::RollbackReserve);
    assert!(entry.flags.has(obc_storage::flat::EntryFlags::RESERVED));
    assert_eq!(entry.payload_len, 0, "the store did not write those bytes and never will");
    assert_eq!(answer.u64_at(8), device.commit_sequence(), "the new catalog commit sequence");
}

#[test]
fn a_refused_package_changes_nothing_and_a_device_with_no_update_path_refuses_every_arm() {
    let disk = formatted_card(22);
    let mut device = boot(&disk);
    let package = payload(4_096);
    let (id, revision) = device.seed(ObjectKind::UpdatePackage, &package, "v2");
    let entries = device.entries().len();
    let free = device.free_extents();

    let mut armer = Armer { refuse: Some(9), ..Armer::default() };
    let wire = device.control_with(&client::arm(1, id, revision), &mut armer);
    assert!(!wire.reboot);
    expect_error(&Answer::of(wire.answer()), ErrorCode::Rejected, 9);
    assert_eq!(device.entries().len(), entries, "no reserve was committed");
    assert_eq!(device.free_extents(), free);

    // The default policy is a device that cannot arm at all.
    let answer = Answer::of(device.control(&client::arm(2, id, revision)).answer());
    assert_eq!(error(&answer).0, ErrorCode::Rejected.value());

    // And a package that is not one.
    let (route, route_revision) = device.seed(ObjectKind::Route, &payload(64), "route");
    let wire = device.control_with(&client::arm(3, route, route_revision), &mut armer);
    expect_error(&Answer::of(wire.answer()), ErrorCode::InvalidRequest, detail::invalid_request::BAD_COMBINATION);
}

#[test]
fn a_malformed_control_record_is_answered_and_an_unanswerable_one_closes_the_stream() {
    let disk = formatted_card(23);
    let mut device = boot(&disk);
    let mut record = client::list(1, None);
    record[3] = b'5';
    let answer = Answer::of(device.control(&record).answer());
    expect_error(&answer, ErrorCode::InvalidFrame, detail::invalid_frame::MAGIC);
    assert_eq!(answer.request, 1, "a response echoes its request");

    let mut zero = client::list(1, None);
    zero[12..16].copy_from_slice(&0u32.to_le_bytes());
    let wire = device.control(&zero);
    assert!(wire.control.is_empty(), "a zero RequestId is unanswerable");
    assert_eq!(wire.closed, Some(obc_link::flat::Channel::Control));
}

#[test]
fn an_unknown_opcode_and_an_unknown_kind_are_unsupported() {
    let disk = formatted_card(24);
    let mut device = boot(&disk);
    let mut record = client::list(1, None);
    record[5] = 0x09;
    expect_error(&Answer::of(device.control(&record).answer()), ErrorCode::Unsupported, detail::unsupported::OPCODE);

    let mut record = client::list(2, None);
    record[4] = 3;
    expect_error(
        &Answer::of(device.control(&record).answer()),
        ErrorCode::Unsupported,
        detail::unsupported::WIRE_MAJOR,
    );

    let record = client::put(3, 0, 0, &payload(64), 99, false, "x");
    expect_error(&Answer::of(device.control(&record).answer()), ErrorCode::Unsupported, detail::unsupported::KIND);
    let _ = UPDATE;
}
