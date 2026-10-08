//! Borrowed, byte-wise views of navigation records. Coordinates are `(lon, lat)` microdegrees.

use super::{
    CHUNK_END, NAV_CHUNK_SIZE, NAV_EDGE_ELEVATION_COMPLETE, NAV_EDGE_FIXED_LEN, NAV_EDGE_POINT_COUNT_MASK,
    NAV_EDGE_PT_COUNT_SENTINEL, NAV_NEIGHBOR_ASCENT_OFF, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN,
};
use crate::io::{rd_i16, rd_i32, rd_u16, rd_u32};

/// One validated edge record. Fields can sit at odd byte offsets.
#[derive(Debug, Clone, Copy)]
pub struct NavEdgeRecord<'a> {
    bytes: &'a [u8],
}

impl<'a> NavEdgeRecord<'a> {
    /// Parse the first record, bounded by the supplied bytes and one navigation chunk.
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < NAV_EDGE_FIXED_LEN {
            return None;
        }
        let count = rd_u16(bytes, 4);
        if count == NAV_EDGE_PT_COUNT_SENTINEL {
            return None;
        }
        let count = (count & NAV_EDGE_POINT_COUNT_MASK) as usize;
        if count < 2 {
            return None;
        }
        let len = NAV_EDGE_FIXED_LEN + (count - 1) * 4;
        if len > NAV_CHUNK_SIZE {
            return None;
        }
        Some(Self { bytes: bytes.get(..len)? })
    }

    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }
    pub fn length_m(self) -> u32 {
        rd_u32(self.bytes, 0)
    }
    pub fn point_count(self) -> usize {
        (self.bytes.len() - NAV_EDGE_FIXED_LEN) / 4 + 1
    }
    pub fn way_kind(self) -> u8 {
        self.bytes[6]
    }
    pub fn elevation_complete(self) -> bool {
        rd_u16(self.bytes, 4) & NAV_EDGE_ELEVATION_COMPLETE != 0
    }
    pub fn anchor(self) -> (i32, i32) {
        (rd_i32(self.bytes, 11), rd_i32(self.bytes, 7))
    }

    /// Forward or reverse vertices without a point buffer. Reverse iteration sums the deltas once.
    pub fn vertices(self) -> NavVertices<'a> {
        NavVertices {
            front: self.anchor(),
            back: None,
            deltas: &self.bytes[NAV_EDGE_FIXED_LEN..],
            remaining: self.point_count(),
        }
    }
}

/// A bounded edge vertex iterator. Mixed forward and reverse calls visit each vertex once.
#[derive(Clone)]
pub struct NavVertices<'a> {
    front: (i32, i32),
    back: Option<(i32, i32)>,
    deltas: &'a [u8],
    remaining: usize,
}

fn step(p: (i32, i32), delta: &[u8], reverse: bool) -> (i32, i32) {
    let (lon, lat) = (rd_i16(delta, 2) as i32, rd_i16(delta, 0) as i32);
    if reverse {
        (p.0.wrapping_sub(lon), p.1.wrapping_sub(lat))
    } else {
        (p.0.wrapping_add(lon), p.1.wrapping_add(lat))
    }
}

impl Iterator for NavVertices<'_> {
    type Item = (i32, i32);
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let p = self.front;
        self.remaining -= 1;
        if self.remaining > 0 {
            self.front = step(p, &self.deltas[..4], false);
            self.deltas = &self.deltas[4..];
        }
        Some(p)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for NavVertices<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let p = *self
            .back
            .get_or_insert_with(|| self.deltas.as_chunks::<4>().0.iter().fold(self.front, |p, d| step(p, d, false)));
        self.remaining -= 1;
        if self.remaining > 0 {
            let at = self.deltas.len() - 4;
            self.back = Some(step(p, &self.deltas[at..], true));
            self.deltas = &self.deltas[..at];
        }
        Some(p)
    }
}
impl ExactSizeIterator for NavVertices<'_> {}
impl core::iter::FusedIterator for NavVertices<'_> {}

/// One adjacency entry with absolute coordinates and unweighted ground length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavNeighbor {
    pub id: u32,
    pub lat: i32,
    pub lon: i32,
    pub edge_id: u32,
    pub cost_m: u32,
    pub way_kind: u8,
    /// Directional climb from this node toward the neighbor.
    pub ascent_m: u16,
}

/// One validated junction record. Neighbor entries decode lazily in record order.
#[derive(Debug, Clone, Copy)]
pub struct NavNodeRecord<'a> {
    pub lat: i32,
    pub lon: i32,
    pub id: u32,
    bytes: &'a [u8],
}

impl<'a> NavNodeRecord<'a> {
    /// Parse the first record. Padding and a truncated record both end a chunk walk.
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < NAV_NODE_FIXED_LEN || bytes[12] == CHUNK_END {
            return None;
        }
        let len = NAV_NODE_FIXED_LEN + bytes[12] as usize * NAV_NEIGHBOR_LEN;
        if len > NAV_CHUNK_SIZE {
            return None;
        }
        Some(Self { lat: rd_i32(bytes, 0), lon: rd_i32(bytes, 4), id: rd_u32(bytes, 8), bytes: bytes.get(..len)? })
    }
    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }
    pub fn degree(&self) -> usize {
        (self.bytes.len() - NAV_NODE_FIXED_LEN) / NAV_NEIGHBOR_LEN
    }
    pub fn neighbors(&self) -> impl Iterator<Item = NavNeighbor> + 'a {
        let (lat, lon) = (self.lat, self.lon);
        self.bytes[NAV_NODE_FIXED_LEN..].as_chunks::<NAV_NEIGHBOR_LEN>().0.iter().map(move |e| NavNeighbor {
            id: rd_u32(e, 0),
            lat: lat.wrapping_add(rd_i16(e, 4) as i32),
            lon: lon.wrapping_add(rd_i16(e, 6) as i32),
            edge_id: rd_u32(e, 8),
            cost_m: rd_u16(e, 12) as u32,
            way_kind: e[14],
            ascent_m: rd_u16(e, NAV_NEIGHBOR_ASCENT_OFF),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn edge() -> Vec<u8> {
        let mut bytes = Vec::from(1234u32.to_le_bytes());
        bytes.extend((8u16 | NAV_EDGE_ELEVATION_COMPLETE).to_le_bytes());
        bytes.push(0x2a);
        bytes.extend(i32::MIN.to_le_bytes());
        bytes.extend(i32::MAX.to_le_bytes());
        for (lat, lon) in [(1i16, 2i16), (-9, 3), (8, -11), (0, 0), (7, 12), (-2, -5), (-5, -1)] {
            bytes.extend(lat.to_le_bytes());
            bytes.extend(lon.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn odd_offset_vertices_wrap_and_mix_both_directions() {
        let mut bytes = std::vec![0];
        bytes.extend(edge());
        let edge = NavEdgeRecord::parse(&bytes[1..]).unwrap();
        assert_eq!((edge.length_m(), edge.way_kind(), edge.elevation_complete()), (1234, 0x2a, true));
        let points = [
            (i32::MAX, i32::MIN),
            (i32::MIN + 1, i32::MIN + 1),
            (i32::MIN + 4, i32::MAX - 7),
            (i32::MAX - 6, i32::MIN),
            (i32::MAX - 6, i32::MIN),
            (i32::MIN + 5, i32::MIN + 7),
            (i32::MIN, i32::MIN + 5),
            (i32::MAX, i32::MIN),
        ];
        assert_eq!(edge.vertices().collect::<Vec<_>>(), points);
        assert_eq!(edge.vertices().rev().collect::<Vec<_>>(), points.into_iter().rev().collect::<Vec<_>>());
        for pattern in 0..256u32 {
            let mut vertices = edge.vertices();
            let (mut front, mut back) = (0, points.len());
            for bit in 0..points.len() {
                assert_eq!(vertices.len(), back - front);
                if pattern & (1 << bit) == 0 {
                    assert_eq!(vertices.next(), Some(points[front]));
                    front += 1;
                } else {
                    back -= 1;
                    assert_eq!(vertices.next_back(), Some(points[back]));
                }
            }
            assert_eq!(vertices.len(), 0);
            assert_eq!(vertices.next(), None);
            assert_eq!(vertices.next_back(), None);
        }
    }

    #[test]
    fn edge_view_refuses_truncation_padding_and_oversize() {
        let bytes = edge();
        for end in 0..bytes.len() {
            assert!(NavEdgeRecord::parse(&bytes[..end]).is_none());
        }
        let mut bytes = bytes;
        for count in [0u16, 1, 0x8000, 0x8001, NAV_EDGE_PT_COUNT_SENTINEL, NAV_EDGE_POINT_COUNT_MASK] {
            bytes[4..6].copy_from_slice(&count.to_le_bytes());
            assert!(NavEdgeRecord::parse(&bytes).is_none());
        }
        bytes.resize(NAV_CHUNK_SIZE + 3, 0);
        bytes[4..6].copy_from_slice(&126u16.to_le_bytes());
        assert!(NavEdgeRecord::parse(&bytes).is_none());
    }

    #[test]
    fn odd_offset_node_neighbors_keep_order_and_directional_climb() {
        let mut bytes = std::vec![0];
        bytes.extend(i32::MAX.to_le_bytes());
        bytes.extend(i32::MIN.to_le_bytes());
        bytes.extend(123u32.to_le_bytes());
        bytes.push(2);
        for (id, dlat, dlon, ascent) in [(124u32, 1i16, -1i16, 5u16), (122, -2, 3, 17)] {
            bytes.extend(id.to_le_bytes());
            bytes.extend(dlat.to_le_bytes());
            bytes.extend(dlon.to_le_bytes());
            bytes.extend(31u32.to_le_bytes());
            bytes.extend(400u16.to_le_bytes());
            bytes.push(0x2a);
            bytes.extend(ascent.to_le_bytes());
        }
        let node = NavNodeRecord::parse(&bytes[1..]).unwrap();
        assert_eq!((node.id, node.lat, node.lon, node.degree()), (123, i32::MAX, i32::MIN, 2));
        assert_eq!(
            node.neighbors().collect::<Vec<_>>(),
            [
                NavNeighbor {
                    id: 124,
                    lat: i32::MIN,
                    lon: i32::MAX,
                    edge_id: 31,
                    cost_m: 400,
                    way_kind: 0x2a,
                    ascent_m: 5
                },
                NavNeighbor {
                    id: 122,
                    lat: i32::MAX - 2,
                    lon: i32::MIN + 3,
                    edge_id: 31,
                    cost_m: 400,
                    way_kind: 0x2a,
                    ascent_m: 17
                },
            ]
        );
        for end in 0..bytes.len() - 1 {
            assert!(NavNodeRecord::parse(&bytes[1..1 + end]).is_none());
        }
        bytes[13] = CHUNK_END;
        assert!(NavNodeRecord::parse(&bytes[1..]).is_none());
        bytes[13] = 30;
        bytes.resize(1 + NAV_NODE_FIXED_LEN + 30 * NAV_NEIGHBOR_LEN, 0);
        assert!(NavNodeRecord::parse(&bytes[1..]).is_none());
    }
}
