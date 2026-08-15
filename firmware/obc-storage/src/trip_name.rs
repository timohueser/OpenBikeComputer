//! Durable and side-loaded trip filename policy.

use crate::object_name;

/// Whether a non-directory FAT entry belongs to the trip catalog.
///
/// The short `.OBT` arm deliberately accepts non-`TP` names as side-loads, preserving the
/// existing catalog rule. A dot-prefixed long name is host clutter and suppresses both arms.
#[inline(always)]
pub fn is_admitted(short_ext: &[u8], long: Option<&str>) -> bool {
    object_name::is_admitted(short_ext, b"OBT", long, b".obt")
}

/// The durable id of an admitted `TP{id}.OBT`, or `None` for a side-loaded trip.
///
/// Directory scans retain only the FAT short name before reopening files. A long-name `.obt`
/// alias cannot carry an uploaded id, so every admitted name except `TP{id}.OBT` is side-loaded.
#[inline(always)]
pub fn uploaded_id(short_base: &[u8], short_ext: &[u8]) -> Option<u16> {
    object_name::uploaded_id(short_base, short_ext, b"TP", b"OBT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploaded_ids_parse_at_band_edges() {
        for (id, name) in [(0, "TP0.OBT"), (7, "TP7.OBT"), (u16::MAX, "TP65535.OBT")] {
            let (base, ext) = name.split_once('.').unwrap();
            assert!(is_admitted(ext.as_bytes(), None));
            assert_eq!(uploaded_id(base.as_bytes(), ext.as_bytes()), Some(id));
        }
    }

    #[test]
    fn side_loaded_short_and_long_names_have_no_durable_id() {
        assert!(is_admitted(b"OBT", None));
        assert_eq!(uploaded_id(b"TOUR", b"OBT"), None);
        assert!(is_admitted(b"OBC", Some("Alpine Tour.ObT")));
        assert_eq!(uploaded_id(b"ALPINE~1", b"OBC"), None);
        assert_eq!(uploaded_id(b"TP65536", b"OBT"), None);
    }

    #[test]
    fn non_trip_and_dot_prefixed_entries_are_rejected() {
        assert!(!is_admitted(b"OBR", None));
        assert!(!is_admitted(b"OBT", Some("._TP3.OBT")));
        assert!(!is_admitted(b"OBC", Some("tour.txt")));
    }
}
