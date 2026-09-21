//! The list-object codecs, with the byte offsets spelled out by hand, not taken from the encoder.

use obc_ble::{ListHeader, RideListEntry, RouteListEntry, TripListEntry};

fn route_entry() -> RouteListEntry<'static> {
    RouteListEntry {
        object_id: 7,
        byte_len: 300,
        distance_m: 2207,
        ascent_m: 76,
        point_count: 9,
        waypoint_count: 2,
        name: "Vector Loop".as_bytes(),
        crc32: 0x1F66_C051,
    }
}

#[test]
fn header_layout_and_roundtrip() {
    // version · entry_len · count LE · total LE
    let b = ListHeader { count: 3, total: 5 }.encode(RouteListEntry::ENTRY_LEN as u8);
    assert_eq!(b, [2, 76, 3, 0, 5, 0]);

    let (h, entry_len) = ListHeader::decode(&b).unwrap();
    assert_eq!(h.count, 3);
    assert_eq!(h.total, 5);
    assert!(h.is_truncated());
    assert_eq!(entry_len, RouteListEntry::ENTRY_LEN);

    assert_eq!(ListHeader::entry_offset(0, entry_len), 6);
    assert_eq!(ListHeader::entry_offset(2, entry_len), 6 + 2 * 76);
    assert_eq!(ListHeader::object_len(3, entry_len), 6 + 3 * 76);

    let full = ListHeader { count: 2, total: 2 }.encode(RideListEntry::ENTRY_LEN as u8);
    assert!(!ListHeader::decode(&full).unwrap().0.is_truncated());
}

#[test]
fn header_rejects_unknown_version_and_short_entries() {
    assert!(ListHeader::decode(&[1, 76, 0, 0, 0, 0]).is_err()); // unknown version
    assert!(ListHeader::decode(&[2, 44, 0, 0, 0, 0]).is_err()); // entry_len below the smallest entry
    assert!(ListHeader::decode(&[2, 76, 0, 0, 0]).is_err()); // truncated

    // A longer entry is legal: readers step by the header's entry_len.
    let (_, entry_len) = ListHeader::decode(&[2, 90, 1, 0, 1, 0]).unwrap();
    assert_eq!(entry_len, 90);
}

#[test]
fn route_entry_layout() {
    let b = route_entry().encode();
    assert_eq!(b.len(), RouteListEntry::ENTRY_LEN);
    assert_eq!(RouteListEntry::ENTRY_LEN, 76, "route catalog entry");
    assert_eq!(&b[0..2], &7u16.to_le_bytes());
    assert_eq!(&b[2..4], &[0, 0]);
    assert_eq!(&b[4..8], &300u32.to_le_bytes());
    assert_eq!(&b[8..12], &2207u32.to_le_bytes());
    assert_eq!(&b[12..16], &76u32.to_le_bytes());
    assert_eq!(&b[16..20], &9u32.to_le_bytes());
    assert_eq!(&b[20..22], &2u16.to_le_bytes());
    assert_eq!(b[22], 11);
    assert_eq!(&b[23..34], b"Vector Loop");
    assert!(b[34..72].iter().all(|&x| x == 0)); // name padding + the reserved tail byte
    assert_eq!(&b[72..76], &0x1F66_C051u32.to_le_bytes()); // content crc32
}

#[test]
fn route_entry_roundtrip_and_truncation() {
    let b = route_entry().encode();
    let d = RouteListEntry::decode(&b).unwrap();
    assert_eq!(d, route_entry());

    let long = [b'x'; 60];
    let e = RouteListEntry { name: &long, ..route_entry() };
    let b = e.encode();
    assert_eq!(b[22], 48);
    assert_eq!(RouteListEntry::decode(&b).unwrap().name.len(), 48);

    let unknown = RouteListEntry { crc32: RouteListEntry::CRC_UNKNOWN, ..route_entry() };
    assert_eq!(RouteListEntry::decode(&unknown.encode()).unwrap().crc32, 0);
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

fn trip_entry() -> TripListEntry<'static> {
    TripListEntry {
        object_id: 1,
        byte_len: 62,
        total_distance_m: 4414,
        total_ascent_m: 152,
        stage_count: 3,
        name: "Alpen Traverse".as_bytes(),
        crc32: 0xDEAD_BEEF,
    }
}

#[test]
fn trip_entry_layout() {
    // Offsets by hand: id, reserved, byte_len, total_distance, total_ascent, stage_count, reserved,
    // name_len, name[48] zero-padded, 3 reserved bytes, then the content crc32.
    let b = trip_entry().encode();
    assert_eq!(b.len(), TripListEntry::ENTRY_LEN);
    assert_eq!(TripListEntry::ENTRY_LEN, 76, "tripList mirrors routeList's v2 core (76 B); it has no expiry tail");
    assert_eq!(&b[0..2], &1u16.to_le_bytes());
    assert_eq!(&b[2..4], &[0, 0]);
    assert_eq!(&b[4..8], &62u32.to_le_bytes());
    assert_eq!(&b[8..12], &4414u32.to_le_bytes());
    assert_eq!(&b[12..16], &152u32.to_le_bytes());
    assert_eq!(&b[16..18], &3u16.to_le_bytes());
    assert_eq!(&b[18..20], &[0, 0]);
    assert_eq!(b[20], 14);
    assert_eq!(&b[21..35], b"Alpen Traverse");
    assert!(b[35..72].iter().all(|&x| x == 0)); // name padding + the 3 reserved tail bytes
    assert_eq!(&b[72..76], &0xDEAD_BEEFu32.to_le_bytes()); // content crc32
}

#[test]
fn trip_entry_roundtrip_and_truncation() {
    let b = trip_entry().encode();
    assert_eq!(TripListEntry::decode(&b).unwrap(), trip_entry());

    let long = [b'x'; 60];
    let e = TripListEntry { name: &long, ..trip_entry() };
    let b = e.encode();
    assert_eq!(b[20], 48);
    assert_eq!(TripListEntry::decode(&b).unwrap().name.len(), 48);

    // stage_count can exceed the stages the totals drew from.
    let unknown = TripListEntry { crc32: TripListEntry::CRC_UNKNOWN, stage_count: 5, ..trip_entry() };
    let encoded = unknown.encode();
    let d = TripListEntry::decode(&encoded).unwrap();
    assert_eq!(d.crc32, 0);
    assert_eq!(d.stage_count, 5);
}

#[test]
fn whole_object_walk() {
    // Build the object as the board does, then walk it as the app does.
    let entries = [route_entry(), RouteListEntry { object_id: 8, name: b"B", ..route_entry() }];
    let mut obj = Vec::new();
    let count = entries.len() as u16;
    obj.extend_from_slice(&ListHeader { count, total: count }.encode(RouteListEntry::ENTRY_LEN as u8));
    for e in &entries {
        obj.extend_from_slice(&e.encode());
    }
    assert_eq!(obj.len(), ListHeader::object_len(entries.len(), RouteListEntry::ENTRY_LEN));

    let (h, entry_len) = ListHeader::decode(&obj).unwrap();
    assert_eq!(h.count as usize, entries.len());
    assert_eq!(entry_len, RouteListEntry::ENTRY_LEN);
    for (k, expected) in entries.iter().enumerate() {
        let slot = ListHeader::entry_slice(&obj, k, entry_len).expect("entry k is in bounds");
        let d = RouteListEntry::decode(slot).unwrap();
        assert_eq!(&d, expected);
    }
    // An entry past the last real one returns None instead of slicing off the end.
    assert!(ListHeader::entry_slice(&obj, entries.len(), entry_len).is_none());
}
