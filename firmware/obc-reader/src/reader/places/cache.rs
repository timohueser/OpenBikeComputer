//! Disposable search work. Losing an arena claim loses speed, never query state.

use super::{nearer, PathProjection, Reach};
use crate::RoutePath;
use core::cell::{Cell, RefCell};
use core::mem::MaybeUninit;
use obc_formats::{
    cache::IndexBlockCache,
    io::{ByteSource, Error},
};

const SLOTS: usize = 384;
const CHUNK_SLOTS: usize = 8;
const CHUNK_POINTS: usize = 256;

struct Chunk {
    generation: u32,
    index: usize,
    len: usize,
    points: [(i32, i32); CHUNK_POINTS],
}

#[derive(Clone, Copy, Default)]
struct Pass {
    along: f32,
    offset: f32,
    present: bool,
    closed: bool,
}
impl Pass {
    fn projection(self) -> Option<PathProjection> {
        self.present.then_some(PathProjection { dist_along_m: self.along, offset_m: self.offset })
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Passes {
    passes: [Pass; 8],
    len: u8,
    pub starts_inside: bool,
    all_inside: bool,
}
impl Passes {
    pub fn new(reach: &Reach<'_>, position: (i32, i32)) -> Option<Self> {
        let mut result =
            Self { starts_inside: reach.inside(reach.points[0], position), all_inside: true, ..Self::default() };
        let mut best = None;
        let mut overflow = false;
        let mut ends_inside = false;
        reach.projections(position, |a_inside, b_inside, projection| {
            result.all_inside &= a_inside && b_inside;
            ends_inside = b_inside;
            if let Some(p) = projection {
                nearer(&mut best, p);
            }
            if !b_inside {
                overflow |= !result.push(best.take(), true);
            }
        });
        if ends_inside {
            overflow |= !result.push(best, false);
        }
        (!overflow).then_some(result)
    }

    fn push(&mut self, projection: Option<PathProjection>, closed: bool) -> bool {
        // Repeated empty boundaries have the same effect as one boundary.
        if closed && projection.is_none() && self.len > 0 {
            let last = self.passes[self.len as usize - 1];
            if last.closed && !last.present {
                return true;
            }
        }
        let Some(slot) = self.passes.get_mut(self.len as usize) else {
            return false;
        };
        *slot = Pass {
            along: projection.map_or(0.0, |p| p.dist_along_m),
            offset: projection.map_or(0.0, |p| p.offset_m),
            present: projection.is_some(),
            closed,
        };
        self.len += 1;
        true
    }

    pub fn preceding_pass(&self, best: &mut Option<PathProjection>) -> bool {
        if let Some(p) = self.passes[..self.len as usize].last().filter(|p| !p.closed).and_then(|p| p.projection()) {
            if best.is_none_or(|old| p.offset_m.abs() <= old.offset_m.abs()) {
                *best = Some(p);
            }
        }
        self.all_inside
    }

    pub fn encounters(
        &self,
        mut skip: bool,
        continuation: bool,
        best: &mut Option<PathProjection>,
        mut emit: impl FnMut(PathProjection),
    ) -> bool {
        for pass in &self.passes[..self.len as usize] {
            if let Some(p) = pass.projection().filter(|_| !skip) {
                nearer(best, p);
            }
            if pass.closed {
                if let Some(p) = best.take() {
                    emit(p);
                }
                skip = false;
                if continuation {
                    break;
                }
            }
        }
        best.is_some()
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Key {
    pub position: (i32, i32),
    pub chunk: u32,
    pub width: u16,
}
impl Key {
    fn slot(self) -> usize {
        let hash = (self.position.0 as u32).wrapping_mul(0x9e3779b9)
            ^ (self.position.1 as u32).rotate_left(13)
            ^ self.chunk.wrapping_mul(0x85ebca6b)
            ^ u32::from(self.width);
        ((hash ^ (hash >> 16)) as usize) % SLOTS
    }
}
#[derive(Clone, Copy, Default)]
struct Slot {
    key: Key,
    value: Passes,
    valid: bool,
}

/// Optional arena-sized cache for map bytes, route points and encounters. Map and route generations
/// invalidate their own entries. No query cursor or user selection lives here.
pub struct PlaceCache {
    map: Cell<u32>,
    route: Cell<u32>,
    bytes: RefCell<MaybeUninit<IndexBlockCache<128>>>,
    passes: RefCell<MaybeUninit<[Slot; SLOTS]>>,
    chunks: RefCell<MaybeUninit<[Chunk; CHUNK_SLOTS]>>,
}
impl PlaceCache {
    /// Initialize in caller-owned storage without putting the cache on the stack.
    ///
    /// # Safety
    /// `dst` must be aligned, writable storage for `Self`, with no live references.
    pub unsafe fn init_in_place(dst: *mut Self) {
        // SAFETY: the caller owns the storage. All-zero is an empty IndexBlockCache; Slot and Chunk contain
        // only integers, floats and booleans. RefCell itself is initialized through its constructor.
        unsafe {
            core::ptr::addr_of_mut!((*dst).map).write(Cell::new(0));
            core::ptr::addr_of_mut!((*dst).route).write(Cell::new(0));
            core::ptr::addr_of_mut!((*dst).bytes).write(RefCell::new(MaybeUninit::uninit()));
            core::ptr::addr_of_mut!((*dst).passes).write(RefCell::new(MaybeUninit::uninit()));
            core::ptr::addr_of_mut!((*dst).chunks).write(RefCell::new(MaybeUninit::uninit()));
            (*dst).bytes.get_mut().as_mut_ptr().write_bytes(0, 1);
            (*dst).passes.get_mut().as_mut_ptr().write_bytes(0, 1);
            (*dst).chunks.get_mut().as_mut_ptr().write_bytes(0, 1);
        }
    }

    pub(in crate::reader) fn adopt_map(&self, generation: u32) {
        if self.map.replace(generation) != generation {
            // SAFETY: init_in_place initializes the whole cache before it is borrowed.
            unsafe { self.bytes.borrow_mut().assume_init_mut() }.reset();
        }
    }
    pub(super) fn get(&self, generation: u32, key: Key) -> Option<Passes> {
        let mut slots = self.passes.borrow_mut();
        // SAFETY: init_in_place initializes every slot.
        let slots = unsafe { slots.assume_init_mut() };
        if self.route.replace(generation) != generation {
            for slot in slots.iter_mut() {
                slot.valid = false;
            }
        }
        let slot = slots[key.slot()];
        (slot.valid && slot.key == key).then_some(slot.value)
    }
    pub(super) fn insert(&self, key: Key, value: Passes) {
        // SAFETY: init_in_place initializes every slot.
        (unsafe { self.passes.borrow_mut().assume_init_mut() })[key.slot()] = Slot { key, value, valid: true };
    }
    pub(in crate::reader) fn read(&self, src: &dyn ByteSource, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        // SAFETY: init_in_place initializes the byte cache.
        unsafe { self.bytes.borrow_mut().assume_init_mut() }.read(src, offset, out, &mut |_, n| n % 8 == 0)
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn visit_points(&self, route: &dyn RoutePath, index: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        let Some(generation) = route.geometry_generation() else {
            return route.visit_chunk_points(index, visit);
        };
        {
            let chunks = self.chunks.borrow();
            // SAFETY: init_in_place initializes every chunk; a zero length means empty.
            let chunk = &unsafe { chunks.assume_init_ref() }[index % CHUNK_SLOTS];
            if chunk.len > 0 && chunk.generation == generation && chunk.index == index {
                visit(&chunk.points[..chunk.len]);
                return;
            }
        }
        route.visit_chunk_points(index, &mut |points| {
            if points.len() <= CHUNK_POINTS {
                if let Ok(mut chunks) = self.chunks.try_borrow_mut() {
                    // SAFETY: init_in_place initializes every chunk.
                    let chunk = &mut unsafe { chunks.assume_init_mut() }[index % CHUNK_SLOTS];
                    chunk.points[..points.len()].copy_from_slice(points);
                    chunk.generation = generation;
                    chunk.index = index;
                    chunk.len = points.len();
                }
            }
            visit(points);
        });
    }
}

const _: () = assert!(core::mem::size_of::<PlaceCache>() <= 128 * 1024);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_passes_preserve_boundaries_ties_and_continuations() {
        for visits in 1..12 {
            let mut points = std::vec::Vec::new();
            for _ in 0..visits {
                points.extend([(0, 0), (0, 500), (0, 0), (0, 10_000)]);
            }
            let reach = Reach::new(&points, 0, 150.0);
            let Some(passes) = Passes::new(&reach, (0, 0)) else {
                assert!(visits >= 8, "overflow must fall back to the geometry walk");
                continue;
            };
            for skip in [false, true] {
                for continuation in [false, true] {
                    let mut expected = std::vec::Vec::new();
                    let mut actual = std::vec::Vec::new();
                    let mut a = Some(PathProjection { dist_along_m: 0.0, offset_m: 100.0 });
                    let mut b = a;
                    assert_eq!(
                        reach.pass_encounters((0, 0), skip, continuation, &mut a, |p| expected.push(p)),
                        passes.encounters(skip, continuation, &mut b, |p| actual.push(p))
                    );
                    assert_eq!(actual, expected);
                    assert_eq!(a, b);
                    assert_eq!(reach.preceding_pass((0, 0), &mut a), passes.preceding_pass(&mut b));
                    assert_eq!(a, b);
                }
            }
        }
    }

    #[test]
    fn generations_invalidate_bytes_and_route_work_independently() {
        let mut cache = std::boxed::Box::<PlaceCache>::new_uninit();
        // SAFETY: the allocation is aligned and exclusively owned.
        let cache = unsafe {
            PlaceCache::init_in_place(cache.as_mut_ptr());
            cache.assume_init()
        };
        let key = Key { position: (0, 0), chunk: 0, width: 150 };
        assert!(cache.get(1, key).is_none());
        cache.insert(key, Passes::default());
        assert!(cache.get(1, key).is_some());
        assert!(cache.get(2, key).is_none());
        let mut out = [0; 8];
        for generation in 1..=2 {
            cache.adopt_map(generation);
            cache.read(&obc_formats::io::SliceSource(&[generation as u8; 512]), 0, &mut out).unwrap();
            assert_eq!(out, [generation as u8; 8]);
        }

        struct Path {
            generation: Cell<u32>,
            reads: Cell<u32>,
            points: std::vec::Vec<(i32, i32)>,
        }
        impl RoutePath for Path {
            fn geometry_generation(&self) -> Option<u32> {
                Some(self.generation.get())
            }
            fn chunk_count(&self) -> usize {
                1
            }
            fn chunk_start_m(&self, _: usize) -> u32 {
                0
            }
            fn chunk_bbox(&self, _: usize) -> obc_map_scene::BBox {
                unreachable!("point-cache reads do not need bounds")
            }
            fn visit_chunk_points(&self, _: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
                self.reads.set(self.reads.get() + 1);
                visit(&self.points);
            }
        }
        let mut path = Path { generation: Cell::new(1), reads: Cell::new(0), points: std::vec![(1, 2); CHUNK_POINTS] };
        for _ in 0..2 {
            cache.visit_points(&path, 0, &mut |points| assert_eq!(points, path.points));
        }
        assert_eq!(path.reads.get(), 1);
        path.generation.set(2);
        path.points[0] = (3, 4);
        cache.visit_points(&path, 0, &mut |points| assert_eq!(points[0], (3, 4)));
        assert_eq!(path.reads.get(), 2);
        path.generation.set(3);
        path.points.push((5, 6));
        for _ in 0..2 {
            cache.visit_points(&path, 0, &mut |points| assert_eq!(points, path.points));
        }
        assert_eq!(path.reads.get(), 4, "larger chunks bypass the cache without truncation");
    }
}
