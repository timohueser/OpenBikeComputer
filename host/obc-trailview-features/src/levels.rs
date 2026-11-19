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

/// The tags of each feature id of the zoom-13 tiles, the number of cells read, and the chains of the tunnel features
/// (`level_tags::chains`). A job reads the cells of one tile, as many jobs at once as `memory` holds.
pub fn tags(
    tiles: &Tiles,
    terrain: &dyn Terrain,
    memory: usize,
) -> (HashMap<u32, Tags>, usize, Vec<level_tags::Chain>) {
    let rules = Rules::embedded();
    let z13 = |c: u32, r: u32| tiles.tiles.get(&(MAX_ZOOM, c, r));
    let mut jobs: Vec<(u32, u32)> =
        (tiles.tiles.iter()).filter(|(k, t)| k.0 == MAX_ZOOM && t.has_ids()).map(|(k, _)| (k.1, k.2)).collect();
    jobs.sort_unstable();
    let done = run(&jobs, memory, |&(c, r)| {
        let mut reader = Reader::new(&z13, terrain, None);
        let n = 1 << (CELL_ZOOM - FEATURE_ZOOM);
        let keys = (r * n..(r + 1) * n).flat_map(|y| (c * n..(c + 1) * n).map(move |x| CellKey { x, y }));
        keys.map(|key| {
            let input = reader.input(key);
            let mut g = graph::build(key, &input, &rules.sections);
            graph::levels(&mut g, &input, &rules.sections, &rules.structures);
            level_tags::found(&g, &input, &rules.sections)
        })
        .collect::<Vec<_>>()
    });
    let id = |[c, r, index]: [u32; 3]| z13(c, r).and_then(|t| t.id(index));
    let mut by_id: BTreeMap<u32, Vec<level_tags::Found>> = BTreeMap::new();
    let (mut groups, mut over): (Vec<Vec<u32>>, Vec<(u32, u32)>) = Default::default();
    let mut links: Vec<level_tags::Link<u32>> = Vec::new();
    for cell in done.into_iter().flatten() {
        for (key, f) in cell.found {
            if let Some(id) = id(key) {
                by_id.entry(id).or_default().push(f);
            }
        }
        groups.extend(cell.groups.into_iter().map(|g| g.into_iter().filter_map(id).collect::<Vec<_>>()));
        over.extend(cell.over.into_iter().filter_map(|(l, u)| Some((id(l)?, id(u)?))));
        links
            .extend(cell.links.into_iter().filter_map(|(a, ma, da, b, mb, db)| Some((id(a)?, ma, da, id(b)?, mb, db))));
    }
    let tags = level_tags::merge(&by_id, &groups, &over, &links, &rules.structures);
    let chains = level_tags::chains(&by_id, &links);
    (tags.into_iter().collect(), jobs.len() << (2 * (CELL_ZOOM - FEATURE_ZOOM)), chains)
}

/// `job` of each of `jobs` on as many threads as `memory` holds jobs of `JOB_BYTES`, in the order of the jobs whatever
/// the order the threads finish them in.
pub(crate) fn run<J: Sync, R: Send>(jobs: &[J], memory: usize, job: impl Fn(&J) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).min(memory / JOB_BYTES).max(1);
    let (next, done) = (AtomicUsize::new(0), Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..threads.min(jobs.len()) {
            scope.spawn(|| {
                while let Some(k) = Some(next.fetch_add(1, Ordering::Relaxed)).filter(|&k| k < jobs.len()) {
                    let out = job(&jobs[k]);
                    done.lock().unwrap().push((k, out));
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_unstable_by_key(|d| d.0);
    done.into_iter().map(|d| d.1).collect()
}

/// The reader of a job's cells: the input of a cell from the zoom-13 tiles of the bake, with the tags of `more` on the
/// features of each id, and from the terrain, as the viewer reads them. It keeps the tiles that it parsed and the
/// pages that it decoded.
pub(crate) struct Reader<'a, 'b> {
    z13: &'b dyn Fn(u32, u32) -> Option<&'a Tile>,
    terrain: &'b dyn Terrain,
    more: Option<&'b HashMap<u32, Tags>>,
    cache: TileCache,
    quadrants: HashMap<(u32, u32, u32), Option<Quadrants>>,
}

impl<'a, 'b> Reader<'a, 'b> {
    pub fn new(
        z13: &'b dyn Fn(u32, u32) -> Option<&'a Tile>,
        terrain: &'b dyn Terrain,
        more: Option<&'b HashMap<u32, Tags>>,
    ) -> Self {
        Reader { z13, terrain, more, cache: TileCache::new(PARSED), quadrants: HashMap::new() }
    }

    pub fn input(&mut self, key: CellKey) -> CellInput {
        let (terrain, quadrants) = (self.terrain, &mut self.quadrants);
        let pages = (Pages::wanted(key).into_iter())
            .filter_map(|page| {
                let (z, tx, ty, q) = page.tile();
                let tile = quadrants.entry((z, tx, ty)).or_insert_with(|| {
                    terrain.tile(z as u8, tx, ty).map(|heights| pages_from_heights(&heights, z, tx, ty))
                });
                tile.as_ref()?[q].clone()
            })
            .collect();
        let (z13, more) = (self.z13, self.more);
        let bytes = |tc: u32, tr: u32| {
            z13(tc, tr).map(|t| match more {
                Some(more) => t.bytes_with(&|id| more.get(&id)),
                None => t.bytes(),
            })
        };
        CellInput::new(key, &mut self.cache, bytes, pages)
            .unwrap_or_else(|e| panic!("the bake's own tile about cell {}/{}: {e:?}", key.x, key.y))
    }
}
