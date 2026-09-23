//! OBCR route-format constants from `OBCR_Spec.md`.

use crate::io::{validate_prefix, DecodeError};

pub const MAGIC: &[u8; 4] = b"OBCR";
/// The one accepted route version. Older files must be re-imported.
pub const VERSION: u8 = 5;
/// The header's ride core — every field the geometry path needs.
pub const HEADER_LEN: usize = 112;
/// The complete fixed header including optional-section metadata.
pub const HEADER_FULL_LEN: usize = 160;
pub const CHUNK_META_LEN: usize = 44;
pub const POINT_RECORD_LEN: usize = 7;
pub const NAME_CAP: usize = 48;
/// Header byte holding the route's [`BikeType`](crate::bike::BikeType).
pub const BIKE_TYPE_OFF: usize = 7;
pub const WAYPOINT_LEN: usize = 80;
pub const WAYPOINT_NAME_CAP: usize = 24;
/// First byte of a waypoint record's name field.
pub const WAYPOINT_NAME_OFF: usize = 20;
pub const ELEVATION_NONE: i16 = i16::MIN;
pub const FLAG_UNRESOLVED_AVOIDANCE: u8 = 1;
pub const FLAG_HAS_ELEVATION: u8 = 2;
pub const FLAG_ATTRIBUTION_MAP: u8 = 4;
pub const FLAG_ASSISTANT_CANDIDATE: u8 = 8;
/// A trip day the device built from the rest of the day before. Lists hide it, and the device keeps
/// at most one, which each new build replaces.
pub const FLAG_BUILT_DAY: u8 = 16;
pub const WAYPOINT_PROVENANCE_OFF: usize = 44;
pub const VISIT_DESCRIPTOR_VERSION: u8 = 1;
pub const VISIT_DESCRIPTOR_LEN: usize = 80;
pub const FACTS_POLICY: u16 = 1;
pub const WAYPOINT_ELE_NONE: i16 = i16::MIN;
/// The waypoint category byte for "no category", the diamond every hand-placed waypoint renders
/// as. `1..=6` are the OBCM `PoiCategory` wire ids; any other value reads as generic.
pub const WAYPOINT_CATEGORY_GENERIC: u8 = 0;

pub const fn is_supported_version(version: u8) -> bool {
    version == VERSION
}

pub fn validate_header_prefix(bytes: &[u8]) -> Result<u8, DecodeError> {
    validate_prefix(bytes, MAGIC, VERSION, VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_committed_route_fixtures() {
        for fixture in [
            &include_bytes!("../../../specs/vectors/route-plain.obcr")[..],
            &include_bytes!("../../../specs/vectors/route-waypoints.obcr")[..],
        ] {
            assert_eq!(validate_header_prefix(fixture), Ok(VERSION));
            assert!(fixture.len() >= HEADER_FULL_LEN);
            assert_eq!(u32::from_le_bytes(fixture[60..64].try_into().unwrap()), HEADER_FULL_LEN as u32);
        }
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(HEADER_FULL_LEN - HEADER_LEN, 48);
        assert_eq!(CHUNK_META_LEN, 4 * 6 + 2 * 2 + 4 * 4);
        assert_eq!(POINT_RECORD_LEN, 2 + 2 + 2 + 1);
        // dist_along · lon · lat · ele · category · name_len · lateral offset · 2 reserved · name
        assert_eq!(WAYPOINT_LEN, 4 + 4 + 4 + 2 + 1 + 1 + 2 + 2 + WAYPOINT_NAME_CAP + 36);
        assert_eq!(WAYPOINT_NAME_OFF, 20);
    }

    /// Old versions are rejected outright.
    #[test]
    fn old_versions_are_rejected() {
        for old in 1..VERSION {
            assert!(!is_supported_version(old));
        }
        assert!(is_supported_version(VERSION));
        let mut v2 = *b"OBCR\x02";
        assert_eq!(validate_header_prefix(&v2), Err(DecodeError::Version));
        v2[4] = VERSION;
        assert_eq!(validate_header_prefix(&v2), Ok(VERSION));
    }
}

/// Exact card-local source identity; owned here to keep route codecs independent of storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSourceKey {
    pub store: [u8; 16],
    pub object: u64,
    pub revision: u64,
}

impl RouteSourceKey {
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != 32 {
            return Err(DecodeError::Bounds);
        }
        if bytes[16..24].iter().all(|b| *b == 0) || bytes[24..32].iter().all(|b| *b == 0) {
            return Err(DecodeError::Bounds);
        }
        Ok(Self {
            store: bytes[..16].try_into().unwrap(),
            object: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            revision: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
        })
    }
    pub fn encode(self, bytes: &mut [u8; 32]) {
        bytes[..16].copy_from_slice(&self.store);
        bytes[16..24].copy_from_slice(&self.object.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.revision.to_le_bytes());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaypointProvenance {
    pub source: RouteSourceKey,
    pub ordinal: u16,
}

impl WaypointProvenance {
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, DecodeError> {
        if bytes.len() != 36 {
            return Err(DecodeError::Bounds);
        }
        match u16::from_le_bytes(bytes[34..36].try_into().unwrap()) {
            0 if bytes.iter().all(|b| *b == 0) => Ok(None),
            1 => Ok(Some(Self {
                source: RouteSourceKey::decode(&bytes[..32])?,
                ordinal: u16::from_le_bytes(bytes[32..34].try_into().unwrap()),
            })),
            _ => Err(DecodeError::Version),
        }
    }
    pub fn encode(self) -> [u8; 36] {
        let mut bytes = [0; 36];
        self.source.encode(bytes[..32].as_mut().try_into().unwrap());
        bytes[32..34].copy_from_slice(&self.ordinal.to_le_bytes());
        bytes[34] = 1;
        bytes
    }
}

/// One accepted visit. Phase persistence belongs to the Navigator checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisitDescriptor {
    pub original: RouteSourceKey,
    pub original_anchors_m: [u32; 3],
    pub accepted_anchors_m: [u32; 3],
    pub target_id: u64,
    pub target_lon: i32,
    pub target_lat: i32,
    /// 1 OSM node, 2 OSM way, 3 OSM relation, 4 Wikidata Q identifier.
    pub target_kind: u8,
}

impl VisitDescriptor {
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != VISIT_DESCRIPTOR_LEN {
            return Err(DecodeError::Bounds);
        }
        if !(1..=4).contains(&bytes[72]) || bytes[73..].iter().any(|b| *b != 0) {
            return Err(DecodeError::Version);
        }
        let mut original_anchors_m = [0; 3];
        let mut accepted_anchors_m = [0; 3];
        for i in 0..3 {
            original_anchors_m[i] = u32::from_le_bytes(bytes[32 + i * 4..36 + i * 4].try_into().unwrap());
            accepted_anchors_m[i] = u32::from_le_bytes(bytes[44 + i * 4..48 + i * 4].try_into().unwrap());
        }
        if !original_anchors_m.windows(2).all(|p| p[0] <= p[1]) || !accepted_anchors_m.windows(2).all(|p| p[0] <= p[1])
        {
            return Err(DecodeError::Bounds);
        }
        let lon = i32::from_le_bytes(bytes[64..68].try_into().unwrap());
        let lat = i32::from_le_bytes(bytes[68..72].try_into().unwrap());
        if !(-180_000_000..=180_000_000).contains(&lon)
            || !(-90_000_000..=90_000_000).contains(&lat)
            || bytes[56..64].iter().all(|b| *b == 0)
        {
            return Err(DecodeError::Bounds);
        }
        Ok(Self {
            original: RouteSourceKey::decode(&bytes[..32])?,
            original_anchors_m,
            accepted_anchors_m,
            target_id: u64::from_le_bytes(bytes[56..64].try_into().unwrap()),
            target_lon: i32::from_le_bytes(bytes[64..68].try_into().unwrap()),
            target_lat: i32::from_le_bytes(bytes[68..72].try_into().unwrap()),
            target_kind: bytes[72],
        })
    }
    pub fn encode(self) -> Result<[u8; VISIT_DESCRIPTOR_LEN], DecodeError> {
        let mut bytes = [0; VISIT_DESCRIPTOR_LEN];
        self.original.encode(bytes[..32].as_mut().try_into().unwrap());
        for i in 0..3 {
            bytes[32 + i * 4..36 + i * 4].copy_from_slice(&self.original_anchors_m[i].to_le_bytes());
            bytes[44 + i * 4..48 + i * 4].copy_from_slice(&self.accepted_anchors_m[i].to_le_bytes());
        }
        bytes[56..64].copy_from_slice(&self.target_id.to_le_bytes());
        bytes[64..68].copy_from_slice(&self.target_lon.to_le_bytes());
        bytes[68..72].copy_from_slice(&self.target_lat.to_le_bytes());
        bytes[72] = self.target_kind;
        Self::decode(&bytes)?;
        Ok(bytes)
    }
}
