//! One index node per step, in NW, NE, SW, SE order. Visited branches must point forward to
//! four in-range children. A branch at depth 32 is malformed. Midpoints use signed floor division.

use super::{MapReadError, MAX_QUADTREE_DEPTH};
use heapless::Vec;
use obc_formats::obcm::{BRANCH_BIT, EMPTY_LEAF};
use obc_map_scene::BBox;

#[derive(Debug, Clone, Copy)]
struct Frame {
    index: [u8; 4],
    quadrant: u8,
}
impl Frame {
    fn new(index: u32, quadrant: u8) -> Self {
        Self { index: index.to_le_bytes(), quadrant }
    }
    fn index(self) -> usize {
        u32::from_le_bytes(self.index) as usize
    }
}

#[derive(Debug, Default)]
pub(super) struct QuadCursor {
    stack: Vec<Frame, 33>,
}

pub(super) enum QuadStep {
    Pending,
    Leaf(u32, BBox),
    Done,
}
pub(super) struct QuadError {
    pub error: MapReadError,
    pub node: BBox,
}

impl QuadCursor {
    pub fn reset(&mut self) {
        self.clear();
        let _ = self.stack.push(Frame::new(0, 4));
    }
    pub fn clear(&mut self) {
        self.stack.clear();
    }
    pub fn step(
        &mut self,
        root: BBox,
        view: &BBox,
        node_count: usize,
        read: impl FnOnce(usize) -> Result<u32, MapReadError>,
    ) -> Result<QuadStep, QuadError> {
        let Some(frame) = self.stack.last().copied() else { return Ok(QuadStep::Done) };
        if node_count == 0 {
            self.clear();
            return Ok(QuadStep::Done);
        }
        let mut node = root;
        for frame in self.stack.iter().skip(1) {
            node = quadrant(node, frame.quadrant);
        }
        if !node.intersects(view) {
            self.advance();
            return Ok(QuadStep::Pending);
        }
        let index = frame.index();
        let result = (|| {
            if index >= node_count {
                return Err(MapReadError::Malformed);
            }
            let value = read(index)?;
            if value & BRANCH_BIT == 0 {
                self.advance();
                return Ok(if value == EMPTY_LEAF { QuadStep::Pending } else { QuadStep::Leaf(value, node) });
            }
            let child = value & !BRANCH_BIT;
            if child as usize <= index
                || child as usize + 3 >= node_count
                || self.stack.len() > MAX_QUADTREE_DEPTH as usize
            {
                return Err(MapReadError::Malformed);
            }
            self.stack.push(Frame::new(child, 0)).map_err(|_| MapReadError::Malformed)?;
            Ok(QuadStep::Pending)
        })();
        result.map_err(|error| {
            self.advance();
            QuadError { error, node }
        })
    }
    fn advance(&mut self) {
        while let Some(frame) = self.stack.pop() {
            if frame.quadrant < 3 {
                let _ = self.stack.push(Frame::new(frame.index() as u32 + 1, frame.quadrant + 1));
                break;
            }
        }
    }
}

fn quadrant(mut node: BBox, quadrant: u8) -> BBox {
    let lon = (i64::from(node.min_lon) + i64::from(node.max_lon)).div_euclid(2) as i32;
    let lat = (i64::from(node.min_lat) + i64::from(node.max_lat)).div_euclid(2) as i32;
    if quadrant & 1 == 0 {
        node.max_lon = lon;
    } else {
        node.min_lon = lon;
    }
    if quadrant < 2 {
        node.min_lat = lat;
    } else {
        node.max_lat = lat;
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::Error as IoError;
    use std::vec::Vec as StdVec;

    fn walk(index: &[u32], root: BBox) -> Result<StdVec<(u32, BBox)>, MapReadError> {
        let mut cursor = QuadCursor::default();
        cursor.reset();
        let mut leaves = StdVec::new();
        loop {
            let mut reads = 0;
            let step = cursor.step(root, &root, index.len(), |i| {
                reads += 1;
                Ok(index[i])
            });
            assert!(reads <= 1);
            match step.map_err(|e| e.error)? {
                QuadStep::Leaf(chunk, node) => leaves.push((chunk, node)),
                QuadStep::Done => return Ok(leaves),
                QuadStep::Pending => {}
            }
        }
    }
    #[test]
    fn ordered_leaves_use_floor_midpoints_without_overflow() {
        let root = BBox { min_lon: i32::MIN, min_lat: i32::MIN, max_lon: i32::MAX, max_lat: i32::MAX };
        assert_eq!(
            walk(&[BRANCH_BIT | 1, 10, 11, 12, 13], root).unwrap(),
            [
                (10, BBox { min_lon: i32::MIN, min_lat: -1, max_lon: -1, max_lat: i32::MAX }),
                (11, BBox { min_lon: -1, min_lat: -1, max_lon: i32::MAX, max_lat: i32::MAX }),
                (12, BBox { min_lon: i32::MIN, min_lat: i32::MIN, max_lon: -1, max_lat: -1 }),
                (13, BBox { min_lon: -1, min_lat: i32::MIN, max_lon: i32::MAX, max_lat: -1 }),
            ]
        );
        assert!(walk(&[], root).unwrap().is_empty());
    }
    fn chain(depth: usize) -> StdVec<u32> {
        let mut index = std::vec![EMPTY_LEAF;4*depth+1];
        for level in 0..depth {
            let at = if level == 0 { 0 } else { 4 * (level - 1) + 1 };
            index[at] = BRANCH_BIT | (4 * level + 1) as u32;
        }
        index[4 * (depth - 1) + 1] = 0;
        index
    }
    #[test]
    fn visited_branches_refuse_backrefs_missing_children_and_excess_depth() {
        let root = BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 };
        for index in [
            std::vec![BRANCH_BIT],
            std::vec![BRANCH_BIT | 1, 0, 0, 0],
            std::vec![BRANCH_BIT | 1, 0, BRANCH_BIT | 1, 0, 0],
            chain(33),
        ] {
            assert_eq!(walk(&index, root), Err(MapReadError::Malformed));
        }
        assert_eq!(walk(&chain(32), root).unwrap(), [(0, root)]);
    }
    #[test]
    fn a_read_failure_consumes_its_subtree_and_allows_the_next_sibling() {
        let root = BBox { min_lon: -100, min_lat: -100, max_lon: 100, max_lat: 100 };
        let index = [BRANCH_BIT | 1, 0, 1, 2, 3];
        let mut cursor = QuadCursor::default();
        cursor.reset();
        assert!(matches!(cursor.step(root, &root, 5, |i| Ok(index[i])), Ok(QuadStep::Pending)));
        let error = match cursor.step(root, &root, 5, |_| Err(MapReadError::Source(IoError::Io))) {
            Err(error) => error,
            _ => panic!("the source failure is retained"),
        };
        assert_eq!(error.error, MapReadError::Source(IoError::Io));
        assert_eq!(error.node, quadrant(root, 0));
        assert!(matches!(
            cursor.step(root, &root, 5, |i| {
                assert_eq!(i, 2);
                Ok(index[i])
            }),
            Ok(QuadStep::Leaf(1, _))
        ));
    }
    #[test]
    fn geometry_margin_errors_do_not_poison_or_mask_the_primary_query() {
        use crate::{MapCache, MapTables, Reader, SliceSource};
        use obcm_testkit::{build_file, LodSpec};
        let bytes = build_file(
            (-100, -100, 100, 100),
            &[],
            &[LodSpec {
                max_mpp: f32::INFINITY,
                chunk_size: 512,
                index: std::vec![BRANCH_BIT | 1, 0, BRANCH_BIT | 2, EMPTY_LEAF, EMPTY_LEAF],
                chunks: std::vec![std::vec![0xff]],
            }],
        );
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let primary = BBox { min_lon: -90, min_lat: 10, max_lon: -1, max_lat: 90 };
        let mut leaves = StdVec::new();
        reader.for_each_chunk(0, &primary, |chunk, _| leaves.push(chunk)).unwrap();
        assert_eq!(leaves, [0]);
        let corrupt = BBox { min_lon: 1, ..primary };
        let corrupt = BBox { max_lon: 90, ..corrupt };
        assert_eq!(reader.for_each_chunk(0, &corrupt, |_, _| {}), Err(MapReadError::Malformed));
    }
}
