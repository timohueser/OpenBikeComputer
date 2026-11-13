//! The levels of the ground plan on the zoom-13 tiles of a bake: the states along each road way, its storey and the
//! slab of its deck on ladder step 2, from `graph::levels` on every cell of each tile with a road way, as
//! `obc_ground_plan::graph::level_tags` writes them. The cells read the tiles and the terrain as the viewer does,
//! with the ground plan's own rule tables.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{CellKey, CELL_ZOOM, FEATURE_ZOOM};
use obc_ground_plan::graph::{self, level_tags};
use obc_ground_plan::terrain::{pages_from_heights, StoredPage, Terrain as Pages};
use obc_ground_plan::tile::TileCache;
use obc_ground_plan::CellInput;

use crate::tiles::{Tile, Tiles, MAX_ZOOM};
use crate::{Tags, Terrain};

/// The tiles that a job keeps parsed: a cell reads its own tile and up to three about it.
const PARSED: usize = 9;
/// The memory of one job at most: its parsed tiles, its pages and the graph of one cell.
const JOB_BYTES: usize = 400 << 20;

/// The decoded pages of a terrain tile.
type Quadrants = [Option<StoredPage>; 4];

/// The tags of each feature id of the zoom-13 tiles, and the number of cells read. A job reads the cells of one
/// tile, as many jobs at once as `memory` holds.
pub fn tags(tiles: &Tiles, terrain: &dyn Terrain, memory: usize) -> (HashMap<u32, Tags>, usize) {
    let rules = Rules::embedded();
    let z13 = |c: u32, r: u32| tiles.tiles.get(&(MAX_ZOOM, c, r));
    let mut jobs: Vec<(u32, u32)> =
        (tiles.tiles.iter()).filter(|(k, t)| k.0 == MAX_ZOOM && t.has_ids()).map(|(k, _)| (k.1, k.2)).collect();
    jobs.sort_unstable();
    let threads = std::thread::available_parallelism().map_or(1, usize::from).min(memory / JOB_BYTES).max(1);
    let (next, done) = (AtomicUsize::new(0), Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..threads.min(jobs.len()) {
            scope.spawn(|| {
                while let Some(&(c, r)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let found = cells_of(c, r, &z13, terrain, rules);
                    done.lock().unwrap().push(found);
                }
            });
        }
    });
    let mut by_id: BTreeMap<u32, Vec<level_tags::Found>> = BTreeMap::new();
    for found in done.into_inner().unwrap() {
        for ([c, r, index], f) in found {
            if let Some(id) = z13(c, r).and_then(|t| t.id(index)) {
                by_id.entry(id).or_default().push(f);
            }
        }
    }
    let tags = by_id.into_iter().map(|(id, found)| (id, level_tags::tags(found.iter())));
    (tags.filter(|(_, t)| !t.is_empty()).collect(), jobs.len() << (2 * (CELL_ZOOM - FEATURE_ZOOM)))
}

/// What the cells of zoom-13 tile (c, r) find, by the feature's tile and position there.
fn cells_of<'a>(
    c: u32,
    r: u32,
    z13: &dyn Fn(u32, u32) -> Option<&'a Tile>,
    terrain: &dyn Terrain,
    rules: &Rules,
) -> Vec<([u32; 3], level_tags::Found)> {
    let (mut cache, mut quadrants) = (TileCache::new(PARSED), HashMap::<(u32, u32, u32), Option<Quadrants>>::new());
    let n = 1 << (CELL_ZOOM - FEATURE_ZOOM);
    let mut out = Vec::new();
    for y in r * n..(r + 1) * n {
        for x in c * n..(c + 1) * n {
            let key = CellKey { x, y };
            let pages = (Pages::wanted(key).into_iter())
                .filter_map(|page| {
                    let (z, tx, ty, q) = page.tile();
                    let tile = quadrants.entry((z, tx, ty)).or_insert_with(|| {
                        terrain.tile(z as u8, tx, ty).map(|heights| pages_from_heights(&heights, z, tx, ty))
                    });
                    tile.as_ref()?[q].clone()
                })
                .collect();
            let input = CellInput::new(key, &mut cache, |tc, tr| z13(tc, tr).map(Tile::bytes), pages)
                .unwrap_or_else(|e| panic!("the bake's own tile about cell {x}/{y}: {e:?}"));
            let mut g = graph::build(key, &input, &rules.sections);
            graph::levels(&mut g, &input, &rules.sections, &rules.structures);
            out.extend(level_tags::found(&g, &input, &rules.sections));
        }
    }
    out
}
