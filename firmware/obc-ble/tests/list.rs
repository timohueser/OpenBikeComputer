//! The §7.4 list-object codecs: byte layout pinned against the spec text (offsets spelled out by
//! hand, not via the encoder), round-trips, name truncation, and the forward-compatibility rules
//! (unknown version rejected, longer `entry_len` stepped over).

use obc_ble::{ListHeader, RideListEntry, RouteListEntry, LIST_ENTRY_LEN};

fn route_entry() -> RouteListEntry<'static> {
    RouteListEntry {
        object_id: 7,
        byte_len: 300,
        distance_m: 2207,
        ascent_m: 76,
        point_count: 9,
        waypoint_count: 2,
        name: "Vector Loop".as_bytes(),
    }
}

#[test]
fn header_layout_and_roundtrip() {
    let b = ListHeader { count: 3 }.encode();
    assert_eq!(b, [1, 72, 3, 0]); // version 1 · entry_len 72 · count LE

    let (h, entry_len) = ListHeader::decode(&b).unwrap();
    assert_eq!(h.count, 3);
    assert_eq!(entry_len, LIST_ENTRY_LEN);

    assert_eq!(ListHeader::entry_offset(0), 4);
    assert_eq!(ListHeader::entry_offset(2), 4 + 144);
    assert_eq!(ListHeader::object_len(3), 4 + 216);
}

#[test]
fn header_rejects_unknown_version_and_short_entries() {
    assert!(ListHeader::decode(&[2, 72, 0, 0]).is_err()); // version 2 unknown
    assert!(ListHeader::decode(&[1, 44, 0, 0]).is_err()); // entries shrank — not append-only
    assert!(ListHeader::decode(&[1, 72, 0]).is_err()); // truncated

    // A *longer* future entry is legal: readers step by the header's entry_len.
    let (_, entry_len) = ListHeader::decode(&[1, 80, 1, 0]).unwrap();
    assert_eq!(entry_len, 80);
}

#[test]
fn route_entry_layout() {
    // Spec §7.4, offsets by hand: id, reserved, byte_len, distance, ascent, points, waypoints,
    // name_len, name[48] zero-padded, trailing reserved byte.
    let b = route_entry().encode();
    assert_eq!(b.len(), LIST_ENTRY_LEN);
    assert_eq!(&b[0..2], &7u16.to_le_bytes());
    assert_eq!(&b[2..4], &[0, 0]);
    assert_eq!(&b[4..8], &300u32.to_le_bytes());
    assert_eq!(&b[8..12], &2207u32.to_le_bytes());
    assert_eq!(&b[12..16], &76u32.to_le_bytes());
    assert_eq!(&b[16..20], &9u32.to_le_bytes());
    assert_eq!(&b[20..22], &2u16.to_le_bytes());
    assert_eq!(b[22], 11);
    assert_eq!(&b[23..34], b"Vector Loop");
    assert!(b[34..].iter().all(|&x| x == 0)); // padding + the reserved tail byte
}

#[test]
fn route_entry_roundtrip_and_truncation() {
    let b = route_entry().encode();
    let d = RouteListEntry::decode(&b).unwrap();
    assert_eq!(d, route_entry());

    // A 60-byte name truncates to the 48-byte cap at encode; the decode reports the stored prefix.
    let long = [b'x'; 60];
    let e = RouteListEntry { name: &long, ..route_entry() };
    let b = e.encode();
    assert_eq!(b[22], 48);
    assert_eq!(RouteListEntry::decode(&b).unwrap().name.len(), 48);
}

#[test]
fn ride_entry_layout_and_roundtrip() {
    let e = RideListEntry {
        object_id: 3,
        byte_len: 74,
        start_time: 1_751_450_000,
        distance_m: 42_500,
        moving_time_s: 9000,
        avg_speed_cms: 472,
        climb_m: 810,
        name: "Höhenweg".as_bytes(),
    };
    let b = e.encode();
    assert_eq!(&b[0..2], &3u16.to_le_bytes());
    assert_eq!(&b[2..4], &[0, 0]);
    assert_eq!(&b[4..8], &74u32.to_le_bytes());
    assert_eq!(&b[8..12], &1_751_450_000u32.to_le_bytes());
    assert_eq!(&b[12..16], &42_500u32.to_le_bytes());
    assert_eq!(&b[16..20], &9000u32.to_le_bytes());
    assert_eq!(&b[20..22], &472u16.to_le_bytes());
    assert_eq!(&b[22..24], &810u16.to_le_bytes());
    assert_eq!(b[24] as usize, "Höhenweg".len()); // UTF-8 bytes, not chars
    assert_eq!(RideListEntry::decode(&b).unwrap(), e);
}

#[test]
fn whole_object_walk() {
    // Build a 2-entry routeList object exactly as the board does (header + packed entries), then
    // walk it as the app will: header first, entries stepped by the announced entry_len.
    let entries = [route_entry(), RouteListEntry { object_id: 8, name: b"B", ..route_entry() }];
    let mut obj = Vec::new();
    obj.extend_from_slice(&ListHeader { count: entries.len() as u16 }.encode());
    for e in &entries {
        obj.extend_from_slice(&e.encode());
    }
    assert_eq!(obj.len(), ListHeader::object_len(entries.len()));

    let (h, entry_len) = ListHeader::decode(&obj).unwrap();
    assert_eq!(h.count as usize, entries.len());
    for (k, expected) in entries.iter().enumerate() {
        let slot = ListHeader::entry_slice(&obj, k, entry_len).expect("entry k is in bounds");
        let d = RouteListEntry::decode(slot).unwrap();
        assert_eq!(&d, expected);
    }
    // A count that overruns the buffer is rejected by the bounds-checked walk, not a panic: an
    // entry past the last real one returns None rather than slicing off the end.
    assert!(ListHeader::entry_slice(&obj, entries.len(), entry_len).is_none());
}
