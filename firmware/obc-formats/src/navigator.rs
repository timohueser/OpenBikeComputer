//! Optional Navigator checkpoint section in the card-local Metadata singleton.
use crate::io::{put_i32, put_u32, rd_i32, rd_u32};

pub const CHECKPOINT_LEN: usize = 96;
pub const CHECKPOINT_VERSION: u16 = 1;

/// Exact payload identity within the StoreId in the enclosing Metadata header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadFingerprint {
    pub object: u64,
    pub revision: u64,
    pub length: u64,
    pub crc: u32,
}

impl PayloadFingerprint {
    pub const fn valid(self) -> bool {
        self.object != 0 && self.revision != 0 && self.length != 0
    }

    fn encode(self, bytes: &mut [u8]) {
        bytes[..8].copy_from_slice(&self.object.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.revision.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.length.to_le_bytes());
        put_u32(bytes, 24, self.crc);
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let value = Self {
            object: u64::from_le_bytes(bytes[..8].try_into().ok()?),
            revision: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            length: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            crc: rd_u32(bytes, 24),
        };
        value.valid().then_some(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JourneyPhase {
    Following = 0,
    Outbound = 1,
    AtStop = 2,
    Returning = 3,
}

/// Recovery offer for the route guidance was following. It does not start navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavigatorCheckpoint {
    pub route: PayloadFingerprint,
    pub original: Option<PayloadFingerprint>,
    pub progress_m: u32,
    pub occurrence: u32,
    pub lon: i32,
    pub lat: i32,
    pub phase: JourneyPhase,
    pub unresolved_avoidance: bool,
    /// The rider's plain route selection rather than a plan they accepted. Its progress is not a
    /// measured anchor, so nothing may re-join the route at it.
    pub selection: bool,
    pub lower_m: u32,
    pub upper_m: u32,
}

impl NavigatorCheckpoint {
    pub fn valid(self) -> bool {
        self.route.valid()
            && self.original.is_none_or(PayloadFingerprint::valid)
            && (self.phase == JourneyPhase::Following || self.original.is_some())
            && (!self.selection || (self.original.is_none() && self.phase == JourneyPhase::Following))
            && (-180_000_000..=180_000_000).contains(&self.lon)
            && (-90_000_000..=90_000_000).contains(&self.lat)
            && self.lower_m <= self.progress_m
            && self.progress_m <= self.upper_m
    }

    pub fn encode(self) -> Option<[u8; CHECKPOINT_LEN]> {
        if !self.valid() {
            return None;
        }
        let mut bytes = [0; CHECKPOINT_LEN];
        self.route.encode(&mut bytes[..28]);
        put_u32(&mut bytes, 28, self.progress_m);
        if let Some(original) = self.original {
            original.encode(&mut bytes[32..60]);
        }
        put_u32(&mut bytes, 60, self.occurrence);
        put_i32(&mut bytes, 64, self.lon);
        put_i32(&mut bytes, 68, self.lat);
        bytes[72] = self.phase as u8;
        bytes[73] = u8::from(self.unresolved_avoidance);
        bytes[74] = u8::from(self.selection);
        put_u32(&mut bytes, 76, self.lower_m);
        put_u32(&mut bytes, 80, self.upper_m);
        Some(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != CHECKPOINT_LEN || bytes[73] > 1 || bytes[74] > 1 || bytes[75] != 0 || bytes[84..] != [0; 12] {
            return None;
        }
        let value = Self {
            route: PayloadFingerprint::decode(&bytes[..28])?,
            original: if bytes[32..60] == [0; 28] { None } else { Some(PayloadFingerprint::decode(&bytes[32..60])?) },
            progress_m: rd_u32(bytes, 28),
            occurrence: rd_u32(bytes, 60),
            lon: rd_i32(bytes, 64),
            lat: rd_i32(bytes, 68),
            phase: match bytes[72] {
                0 => JourneyPhase::Following,
                1 => JourneyPhase::Outbound,
                2 => JourneyPhase::AtStop,
                3 => JourneyPhase::Returning,
                _ => return None,
            },
            unresolved_avoidance: bytes[73] == 1,
            selection: bytes[74] == 1,
            lower_m: rd_u32(bytes, 76),
            upper_m: rd_u32(bytes, 80),
        };
        value.valid().then_some(value)
    }
}
