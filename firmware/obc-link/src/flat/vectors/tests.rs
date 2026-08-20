//! The loop the fixtures close: the files on disk are what the producer emits, the codec agrees
//! with those bytes in both directions, and every refusal lands where §3.9 says it does.

use std::string::String;
use std::vec::Vec;

use super::*;
use crate::flat::ids::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision, StoreId};
use crate::flat::wire::{
    decode_request, encode_arm, encode_cancel, encode_error, encode_format, encode_get, encode_put, encode_remove,
    encode_status, write_stream, ControlError, ListWriter, ObjectState, Opcode, Refusal, RequestId, StatusResponse,
    StreamFrame, CONTROL_FLOOR,
};

/// Rewrites `specs/vectors/flat-store-v4/`.
///
/// Deliberately `#[ignore]`d: fixtures move only when a human decided they should, and the guard
/// below is what fails when they moved without one.
#[test]
#[ignore = "writes the checked-in fixture suite"]
fn flat_regenerate() {
    let written = write_all().expect("the suite writes");
    std::println!("wrote {written} files to {}", dir().display());
}

fn find(name: &str) -> Fixture {
    fixtures().into_iter().find(|fixture| fixture.name == name).unwrap_or_else(|| panic!("no fixture named {name}"))
}

fn is_request(fixture: &Fixture) -> bool {
    fixture.json.contains("\"direction\": \"request\"")
}

#[test]
fn checked_in_fixtures_match_the_producer() {
    let root = dir();
    let all = fixtures();
    for fixture in &all {
        let path = root.join(fixture.path());
        let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "fixture {} unreadable ({error}) — run `cargo test -p obc-link flat_regenerate -- --ignored`",
                path.display()
            )
        });
        assert_eq!(
            checked_in,
            fixture.json,
            "fixture drift in {} — run `cargo test -p obc-link flat_regenerate -- --ignored` if the change is \
             deliberate",
            fixture.path()
        );
    }
    let checked_in = std::fs::read_to_string(root.join("manifest.json")).expect("manifest.json");
    assert_eq!(checked_in, manifest(&all), "manifest drift — the CI guard exists to catch exactly this");
}

#[test]
fn every_fixture_has_a_unique_name_and_a_digest_in_the_manifest() {
    let all = fixtures();
    let mut names: Vec<String> = all.iter().map(|fixture| fixture.name.clone()).collect();
    names.sort();
    let unique = names.len();
    names.dedup();
    assert_eq!(names.len(), unique, "two fixtures share a name");
    let rendered = manifest(&all);
    for fixture in &all {
        assert_eq!(fixture.sha256().len(), 64);
        assert!(rendered.contains(&fixture.sha256()), "{} is missing from the manifest", fixture.name);
    }
    // The suite's own size, so a category that stopped being produced is visible.
    let count = |category: Category| all.iter().filter(|fixture| fixture.category == category).count();
    assert_eq!(count(Category::Control), 25);
    assert_eq!(count(Category::Stream), 4);
    assert_eq!(count(Category::Error), 14);
    assert_eq!(count(Category::Negative), 25);
}

#[test]
fn the_codec_decodes_every_positive_request_vector() {
    for fixture in fixtures().into_iter().filter(|fixture| fixture.category == Category::Control && is_request(fixture))
    {
        let decoded = decode_request(&fixture.bytes);
        assert!(decoded.is_ok(), "{} did not decode: {decoded:?}", fixture.name);
    }
    for fixture in fixtures().into_iter().filter(|fixture| fixture.category == Category::Stream) {
        let split = StreamFrame::split(&fixture.bytes);
        let (frame, payload) = split.unwrap_or_else(|| panic!("{} did not split", fixture.name));
        assert_eq!(payload.len(), frame.len as usize);
        assert_eq!(&fixture.bytes[..16], &frame.encode(), "{} re-encodes to other bytes", fixture.name);
    }
}

/// §5.7's route entry, as a `LIST` page carries it.
fn route_entry() -> EntryMeta {
    EntryMeta {
        id: ObjectId(ROUTE_ID),
        revision: Revision(ROUTE_REVISION),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: ROUTE_LEN,
        payload_crc: ROUTE_CRC,
        name: DisplayName::from_bytes(ROUTE_NAME).unwrap(),
    }
}

/// §5.7's recording ride.
fn ride_entry() -> EntryMeta {
    EntryMeta {
        id: ObjectId(RIDE_ID),
        revision: Revision(RIDE_REVISION),
        kind: ObjectKind::Ride,
        flags: EntryFlags::RECORDING,
        payload_len: 0,
        payload_crc: 0,
        name: DisplayName::default(),
    }
}

fn list_page(ceiling: usize, sequence: u64, entries: &[EntryMeta], more: bool, request: u32) -> Vec<u8> {
    let mut out = std::vec![0u8; 4_096];
    let mut writer = ListWriter::start(&mut out, ceiling, StoreId(STORE), sequence).expect("a page above the floor");
    for entry in entries {
        assert!(writer.push(&mut out, entry), "the ceiling does not carry the page under test");
    }
    let len = writer.finish(&mut out, RequestId(request), more).expect("the page seals");
    out.truncate(len);
    out
}

#[test]
fn the_codec_encodes_every_response_vector_byte_for_byte() {
    let mut out = std::vec![0u8; 4_096];
    let encoded = |name: &str, bytes: &[u8]| {
        assert_eq!(hex(bytes), hex(&find(name).bytes), "{name} is not what the codec produces");
    };

    encoded(
        "list-response-two-entries",
        &list_page(4_096, SEQUENCE, &[route_entry(), ride_entry()], false, LIST_REQUEST),
    );
    encoded("list-response-empty-catalog", &list_page(CONTROL_FLOOR, 1, &[], false, LIST_REQUEST));
    encoded(
        "list-response-with-a-further-page",
        &list_page(CONTROL_FLOOR, SEQUENCE, &[route_entry()], true, LIST_REQUEST),
    );

    for (name, state, revision, len, crc) in [
        ("status-response-committed", ObjectState::Committed, ROUTE_REVISION, ROUTE_LEN, ROUTE_CRC),
        ("status-response-superseded", ObjectState::Superseded, ROUTE_REVISION + 1, ROUTE_LEN, ROUTE_CRC),
        ("status-response-absent", ObjectState::Absent, 0, 0, 0),
    ] {
        let answer = StatusResponse { state, revision: Revision(revision), payload_len: len, payload_crc: crc };
        let written = encode_status(&mut out, RequestId(0x0000_2A03), &answer).unwrap();
        encoded(name, &out[..written]);
    }

    let written = encode_get(&mut out, RequestId(0x0000_2A04), Revision(ROUTE_REVISION), ROUTE_LEN, ROUTE_CRC).unwrap();
    encoded("get-response", &out[..written]);

    let written =
        encode_put(&mut out, RequestId(UPLOAD_REQUEST), ObjectId(ROUTE_ID), Revision(1), ROUTE_LEN, ROUTE_CRC).unwrap();
    encoded("put-response", &out[..written]);

    let written = encode_remove(&mut out, RequestId(0x0000_2A05), SEQUENCE + 1).unwrap();
    encoded("remove-response", &out[..written]);

    let written = encode_cancel(&mut out, RequestId(0x0000_2A06), true).unwrap();
    encoded("cancel-response-cancelled", &out[..written]);
    let written = encode_cancel(&mut out, RequestId(0x0000_2A06), false).unwrap();
    encoded("cancel-response-no-such-transfer", &out[..written]);

    let written = encode_arm(&mut out, RequestId(0x0000_2A07), ObjectId(6), SEQUENCE + 1).unwrap();
    encoded("arm-response", &out[..written]);

    let written = encode_format(&mut out, RequestId(0x0000_2A08), StoreId(REPLACEMENT_STORE)).unwrap();
    encoded("format-response", &out[..written]);

    let written = write_stream(&mut out, RequestId(UPLOAD_REQUEST), 40_960, 1_024).unwrap();
    assert_eq!(hex(&out[..16]), hex(&find("stream-frame-of-section-3-11").bytes[..16]));
    assert_eq!(written, 16 + 1_024);
}

#[test]
fn every_error_vector_is_what_the_codec_encodes_for_that_refusal() {
    let mut out = std::vec![0u8; 64];
    for fixture in fixtures().into_iter().filter(|fixture| fixture.category == Category::Error) {
        let refusal = Refusal::decode(&fixture.bytes[16..]).unwrap_or_else(|| panic!("{} has no body", fixture.name));
        let opcode = Opcode::decode(fixture.bytes[5]).expect("a registered opcode");
        let request = RequestId(u32::from_le_bytes(fixture.bytes[12..16].try_into().unwrap()));
        let written = encode_error(&mut out, opcode, request, &refusal).unwrap();
        assert_eq!(hex(&out[..written]), hex(&fixture.bytes), "{} is not what the codec produces", fixture.name);
    }
    // Every code §3.9 registers has a vector, and none has two.
    let mut codes: Vec<u16> = fixtures()
        .iter()
        .filter(|fixture| fixture.category == Category::Error)
        .map(|fixture| u16::from_le_bytes([fixture.bytes[16], fixture.bytes[17]]))
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, (1..=14u16).collect::<Vec<_>>());
}

#[test]
fn every_negative_vector_is_refused_with_its_stated_code_and_detail() {
    for fixture in fixtures().into_iter().filter(|fixture| fixture.category == Category::Negative) {
        if fixture.json.contains("\"target\": \"streamRecord\"") {
            assert!(StreamFrame::split(&fixture.bytes).is_none(), "{} split", fixture.name);
            continue;
        }
        let outcome = decode_request(&fixture.bytes);
        if fixture.json.contains("\"disposition\": \"closeRecordStream\"") {
            assert_eq!(outcome, Err(ControlError::Unanswerable), "{} is answerable", fixture.name);
            continue;
        }
        let Err(ControlError::Refused { refusal, .. }) = outcome else {
            panic!("{} was accepted: {outcome:?}", fixture.name)
        };
        let stated = |key: &str| -> u16 {
            let at = fixture.json.find(key).unwrap_or_else(|| panic!("{} has no {key}", fixture.name)) + key.len() + 2;
            fixture.json[at..].split([',', '\n']).next().unwrap().trim().parse().expect("a number")
        };
        assert_eq!(refusal.code.value(), stated("\"codeValue\""), "{} refused with another code", fixture.name);
        assert_eq!(refusal.detail, stated("\"detailValue\""), "{} refused with another detail", fixture.name);
    }
}

/// Transcribes a spec hex fence.
fn spec_hex(text: &str) -> Vec<u8> {
    text.split('\n')
        .flat_map(|line| line.split_whitespace().skip(1))
        .map(|token| u8::from_str_radix(token, 16).expect("a hex byte pair"))
        .collect()
}

#[test]
fn section_3_11s_own_frames_are_in_the_suite_verbatim() {
    let put = spec_hex(
        "0000  4F 42 43 34 04 04 00 00 54 00 00 00 01 2A 00 00
         0010  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         0020  99 A4 00 00 00 00 00 00 21 7E 4A 9C 01 00 00 00
         0030  0C 00 00 00 47 72 69 6D 73 65 6C 20 4C 6F 6F 70
         0040  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         0050  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         0060  00 00 00 00",
    );
    assert_eq!(find("put-create-request").bytes, put, "§3.11's PUT is not the suite's");

    let stream = spec_hex("0000  01 2A 00 00 00 A0 00 00 00 00 00 00 00 04 00 00");
    assert_eq!(find("stream-frame-of-section-3-11").bytes[..16], stream[..], "§3.11's stream frame is not the suite's");

    let conflict = spec_hex(
        "0000  4F 42 43 34 04 04 03 00 10 00 00 00 01 2A 00 00
         0010  05 00 01 00 05 00 00 00 00 00 00 00 00 00 00 00",
    );
    assert_eq!(find("revision-conflict-head-differs").bytes, conflict, "§3.11's error response is not the suite's");

    let list = spec_hex(
        "0000  4F 42 43 34 04 01 01 00 C8 00 00 00 02 2A 00 00
         0010  8F 2C 41 D9 6B 07 4E A3 B1 55 9C 20 7D E8 34 66
         0020  07 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00
         0030  03 00 00 00 00 00 00 00 99 A4 00 00 00 00 00 00
         0040  21 7E 4A 9C 01 00 00 00 0C 00 00 00 47 72 69 6D
         0050  73 65 6C 20 4C 6F 6F 70 00 00 00 00 00 00 00 00
         0060  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         0070  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         0080  02 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00
         0090  00 00 00 00 00 00 00 00 00 00 00 00 03 00 01 00
         00a0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         00b0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         00c0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         00d0  00 00 00 00 00 00 00 00",
    );
    assert_eq!(find("list-response-two-entries").bytes, list, "§3.11's LIST response is not the suite's");
    assert_eq!(list.len(), 216, "the spec calls it 216 bytes");
}

#[test]
fn the_producers_crc_is_the_contracts_crc() {
    assert_eq!(raw::crc32(b"123456789"), 0xCBF4_3926);
    let mut theirs = obc_crc::Crc32::new();
    theirs.update(b"a flat store");
    assert_eq!(raw::crc32(b"a flat store"), theirs.finalize());
}
