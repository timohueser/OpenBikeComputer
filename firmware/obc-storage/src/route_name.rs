//! Durable and side-loaded route filename policy.

use crate::object_name;

/// Whether a non-directory FAT entry belongs to the route catalog.
///
/// Side-loaded routes use a long `.obcr` filename. Device uploads use the FAT-short `*.OBR`
/// twin because the device cannot create long filenames. A dot-prefixed long name is host clutter
/// and suppresses both arms.
#[inline(always)]
pub fn is_admitted(short_ext: &[u8], long: Option<&str>) -> bool {
    object_name::is_admitted(short_ext, b"OBR", long, b".obcr")
}

/// The durable id of an admitted `RT{id}.OBR`, or `None` for a side-loaded route.
#[inline(always)]
pub fn uploaded_id(short_base: &[u8], short_ext: &[u8]) -> Option<u16> {
    object_name::uploaded_id(short_base, short_ext, b"RT", b"OBR")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploaded_ids_parse_at_band_edges() {
        for (id, base) in [(0, b"RT0".as_slice()), (u16::MAX, b"RT65535".as_slice())] {
            assert!(is_admitted(b"OBR", None));
            assert_eq!(uploaded_id(base, b"OBR"), Some(id));
        }
    }

    #[test]
    fn malformed_or_overflowing_uploaded_ids_are_rejected() {
        for base in [b"RT".as_slice(), b"RT-1", b"RT1A", b"RT65536", b"RT99999999", b"RX7"] {
            assert_eq!(uploaded_id(base, b"OBR"), None);
        }
        assert_eq!(uploaded_id(b"RT7", b"OBT"), None);
        assert_eq!(uploaded_id(b"rt7", b"OBR"), None);
    }

    #[test]
    fn short_and_long_side_loads_have_no_durable_id() {
        assert!(is_admitted(b"OBR", None));
        assert_eq!(uploaded_id(b"TOUR", b"OBR"), None);
        assert!(is_admitted(b"OBC", Some("Alpine Pass.ObCr")));
        assert_eq!(uploaded_id(b"ALPINE~1", b"OBC"), None);
    }

    #[test]
    fn dot_clutter_and_wrong_extensions_are_rejected() {
        assert!(!is_admitted(b"OBR", Some("._RT7.OBR")));
        assert!(!is_admitted(b"OBC", Some(".hidden.obcr")));
        assert!(!is_admitted(b"OBT", None));
        assert!(!is_admitted(b"OBC", Some("route.txt")));
        assert!(!is_admitted(b"OBC", Some("route.obcr.bak")));
    }
}
