//! The decoded shapes of the last level-1 cells that one worker snapped in, with the box of each
//! road, so that a snap near an earlier one decodes no road again.

use crate::{model::Point, Result};
use std::{collections::VecDeque, mem::size_of};

/// The most bytes of decoded shapes that one `Region` holds, unless one cell alone is larger.
pub(crate) const SNAP_CACHE_BYTES: usize = 4 << 20;
/// The most cells that the cache holds, so that a lookup stays a short scan.
const SNAP_CACHE_CELLS: usize = 256;

/// The shapes of the roads of one level-1 cell, one after the other.
#[derive(Default)]
pub(crate) struct CellShapes {
    points: Vec<Point>,
    /// For each road, the end of its points and its box (west, south, east, north).
    roads: Vec<(u32, [i32; 4])>,
}

impl CellShapes {
    /// Appends the shape of the next road.
    pub(crate) fn push(&mut self, shape: &[Point]) {
        let mut bounds = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for point in shape {
            bounds = [
                bounds[0].min(point.lon),
                bounds[1].min(point.lat),
                bounds[2].max(point.lon),
                bounds[3].max(point.lat),
            ];
        }
        self.points.extend_from_slice(shape);
        self.roads.push((self.points.len() as u32, bounds));
    }

    pub(crate) fn len(&self) -> u32 {
        self.roads.len() as u32
    }

    /// The shape and the box of road `k` of the cell.
    pub(crate) fn road(&self, k: u32) -> (&[Point], [i32; 4]) {
        let k = k as usize;
        let start = k.checked_sub(1).map_or(0, |before| self.roads[before].0 as usize);
        let (end, bounds) = self.roads[k];
        (&self.points[start..end as usize], bounds)
    }

    fn bytes(&self) -> usize {
        self.points.capacity() * size_of::<Point>() + self.roads.capacity() * size_of::<(u32, [i32; 4])>()
    }
}

/// The decoded level-1 cells by the record of their download cell and their index in it, the
/// least recently used first.
#[derive(Default)]
pub(crate) struct Shapes {
    cells: VecDeque<((u32, u32), CellShapes)>,
    bytes: usize,
}

impl Shapes {
    /// The cell of `key`; `fill` decodes it on a miss. A failed fill keeps nothing.
    pub(crate) fn get(
        &mut self,
        key: (u32, u32),
        fill: impl FnOnce(&mut CellShapes) -> Result<()>,
    ) -> Result<&CellShapes> {
        let found = self.cells.iter().position(|(other, _)| *other == key);
        if let Some(entry) = found.and_then(|k| self.cells.remove(k)) {
            self.cells.push_back(entry);
        } else {
            let mut cell = CellShapes::default();
            fill(&mut cell)?;
            cell.points.shrink_to_fit();
            cell.roads.shrink_to_fit();
            self.bytes += cell.bytes();
            self.cells.push_back((key, cell));
            while self.cells.len() > 1 && (self.bytes > SNAP_CACHE_BYTES || self.cells.len() > SNAP_CACHE_CELLS) {
                if let Some((_, old)) = self.cells.pop_front() {
                    self.bytes -= old.bytes();
                }
            }
        }
        Ok(&self.cells[self.cells.len() - 1].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    /// A fill of one road of `points` points.
    fn road(points: usize) -> impl FnOnce(&mut CellShapes) -> Result<()> {
        move |cell| {
            cell.push(&vec![Point { lat: 1, lon: 2, elevation: 0.0 }; points]);
            Ok(())
        }
    }

    #[test]
    fn the_cache_keeps_its_bounds_and_drops_the_least_recently_used_cell() {
        let third = (SNAP_CACHE_BYTES / 3 - size_of::<(u32, [i32; 4])>()) / size_of::<Point>();
        let mut shapes = Shapes::default();
        let keys = |shapes: &Shapes| shapes.cells.iter().map(|(key, _)| key.1).collect::<Vec<_>>();
        for k in 0..3 {
            shapes.get((0, k), road(third)).unwrap();
        }
        // A hit decodes nothing and makes the cell the most recently used.
        let cell = shapes.get((0, 0), |_| Err(Error::InvalidData("decoded again".into()))).unwrap();
        assert_eq!((cell.len(), cell.road(0).0.len(), cell.road(0).1), (1, third, [2, 1, 2, 1]));
        shapes.get((0, 3), road(third)).unwrap();
        assert_eq!(keys(&shapes), [2, 0, 3]);
        assert!(shapes.bytes <= SNAP_CACHE_BYTES);
        // A cell above the bound stays alone.
        shapes.get((0, 4), road(SNAP_CACHE_BYTES / size_of::<Point>() + 1)).unwrap();
        assert_eq!(keys(&shapes), [4]);
        // A failed fill keeps nothing.
        assert!(shapes.get((0, 5), |_| Err(Error::InvalidData("bad".into()))).is_err());
        assert_eq!(keys(&shapes), [4]);
        // The count is bounded too.
        for k in 0..=SNAP_CACHE_CELLS as u32 {
            shapes.get((1, k), road(2)).unwrap();
        }
        assert_eq!(shapes.cells.len(), SNAP_CACHE_CELLS);
        assert_eq!(shapes.cells.front().unwrap().0, (1, 1));
    }
}
