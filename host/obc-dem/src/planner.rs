//! The heights of the planner: its terrain tiles and the slopes of its routes. The reference
//! archive gives the height where it holds the ground, and GLO-30 elsewhere.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use obc_data::store::hash_file;
use rayon::prelude::*;

use crate::fetch::TileId;
use crate::geotiff::{DemMosaic, DemTile};
use crate::reference::{ReferenceArchive, ReferenceTile, TileLookup, Window};

/// The GLO-30 squares that `bounds` (west, south, east, north in degrees) touches, south to north,
/// then west to east: the order of [`Terrain::identities`].
pub fn tiles([west, south, east, north]: [f64; 4]) -> Vec<TileId> {
    let lats = south.floor() as i32..=north.floor() as i32;
    lats.flat_map(|lat| (west.floor() as i32..=east.floor() as i32).map(move |lon| TileId { lat, lon })).collect()
}

/// The GLO-30 files of `bounds` in `dir`: a square without a file is sea.
pub fn tiles_in(dir: &Path, bounds: [f64; 4]) -> Vec<(TileId, PathBuf)> {
    let files = tiles(bounds).into_iter().map(|tile| (tile, dir.join(tile.file_name())));
    files.filter(|(_, path)| path.is_file()).collect()
}

/// The GLO-30 squares, by latitude, whose posts a height at `lat` in microdegrees can read: its
/// bilinear corners lie within two posts, and a square holds the posts of its own degree.
pub fn squares(lat: i32) -> RangeInclusive<i32> {
    const SLACK: i32 = 10_000;
    (lat - SLACK).div_euclid(1_000_000)..=(lat + SLACK).div_euclid(1_000_000)
}

/// The reference tiles that the last heights read. A caller that samples near its last sample
/// keeps one, so a reference tile decodes once for many heights.
#[derive(Default)]
pub struct Recent(VecDeque<((u32, u32), Option<ReferenceTile>)>);

pub struct Terrain {
    /// The GLO-30 files by the latitude of their square.
    glo30: BTreeMap<i32, Vec<PathBuf>>,
    /// The decoded squares of the latitudes in `loaded`.
    fallback: DemMosaic,
    loaded: BTreeSet<i32>,
    reference: Option<ReferenceArchive>,
    /// The SHA-256 of each GLO-30 file, then of each reference tile that `bounds` reaches.
    pub identities: Vec<String>,
    used: Mutex<BTreeSet<String>>,
    /// The credit of GLO-30.
    credit: String,
}

impl Terrain {
    /// The GLO-30 files `glo30`, in the order of [`tiles`], with their `credit`, and the reference
    /// archive at `reference`, for heights in `bounds`. Each file is hashed and its header checked;
    /// no square is decoded until [`Terrain::load`].
    pub fn open(
        glo30: &[(TileId, PathBuf)],
        credit: &str,
        reference: Option<&Path>,
        bounds: [f64; 4],
    ) -> Result<Self, String> {
        let hashes = glo30.par_iter().map(|(_, path)| {
            DemTile::check(path)?;
            hash_file(path).map(|(sha256, _)| sha256).map_err(|e| format!("{}: {e}", path.display()))
        });
        let mut identities = hashes.collect::<Result<Vec<_>, _>>()?;
        let mut files = BTreeMap::<_, Vec<_>>::new();
        for (tile, path) in glo30 {
            files.entry(tile.lat).or_default().push(path.clone());
        }
        let reference = reference.map(ReferenceArchive::open).transpose()?;
        if let Some(archive) = &reference {
            let window = Window {
                lat_lo: (bounds[1] * 1e6) as i64,
                lat_hi: (bounds[3] * 1e6) as i64 + 1,
                lon_lo: (bounds[0] * 1e6) as i64,
                lon_hi: (bounds[2] * 1e6) as i64 + 1,
            };
            identities.extend(archive.held_digests(window).into_iter().map(|(_, _, hash)| hash.to_owned()));
        }
        let (fallback, loaded, used) = (DemMosaic::default(), BTreeSet::new(), Mutex::default());
        Ok(Self { glo30: files, fallback, loaded, reference, identities, used, credit: credit.into() })
    }

    /// Decode the GLO-30 squares of the latitudes `lats` and drop every other square.
    pub fn load(&mut self, lats: RangeInclusive<i32>) -> Result<(), String> {
        let missing: Vec<_> = self
            .glo30
            .range(lats.clone())
            .filter(|(lat, _)| !self.loaded.contains(lat))
            .flat_map(|(_, paths)| paths)
            .collect();
        let decoded = missing.into_par_iter().map(|path| DemTile::open(path)).collect::<Result<Vec<_>, _>>()?;
        // A GLO-30 square holds the posts north of its south edge, up to and with the next degree.
        self.fallback.retain(|tile| lats.contains(&(tile.south_west_deg().0.floor() as i32)));
        decoded.into_iter().for_each(|tile| self.fallback.push(tile));
        self.loaded = self.glo30.range(lats).map(|(lat, _)| *lat).collect();
        Ok(())
    }

    /// The height in metres at `lat`, `lon` in microdegrees, or `None` where no source has one.
    /// GLO-30 answers from the squares that [`Terrain::load`] decoded, which must hold
    /// [`squares`] of `lat`.
    pub fn height(&self, lat: i32, lon: i32, recent: &mut Recent) -> Result<Option<f64>, String> {
        debug_assert!(
            squares(lat).all(|lat| self.loaded.contains(&lat) || !self.glo30.contains_key(&lat)),
            "the GLO-30 squares of {lat} µdeg are not loaded"
        );
        let mut best = None;
        if let Some(archive) = &self.reference {
            let window = Window {
                lat_lo: lat as i64 - 32,
                lat_hi: lat as i64 + 32,
                lon_lo: lon as i64 - 32,
                lon_hi: lon as i64 + 32,
            };
            let cache = &mut recent.0;
            for key in window.tiles() {
                let tile = if let Some(index) = cache.iter().position(|(id, _)| id == &key) {
                    cache.remove(index).unwrap().1
                } else {
                    match archive.tile(key.0, key.1)? {
                        TileLookup::Held { tile, sources } => {
                            self.used.lock().unwrap().extend(sources.iter().cloned());
                            Some(tile)
                        }
                        TileLookup::Absent | TileLookup::Unknown => None,
                    }
                };
                if let Some(tile) = &tile {
                    tile.centres_in(window, |_, _, metres| best = Some(metres as f64));
                }
                cache.push_back((key, tile));
                if cache.len() > 32 {
                    cache.pop_front();
                }
            }
        }
        Ok(best.or_else(|| self.fallback.height(lat as f64 * 1e-6, lon as f64 * 1e-6)))
    }

    /// The credits of the sources that gave a height so far.
    pub fn attribution(&self) -> Vec<String> {
        let mut credits = Vec::new();
        if let Some(archive) = &self.reference {
            let used = self.used.lock().unwrap();
            credits.extend(
                archive
                    .credits()
                    .iter()
                    .filter(|c| used.contains(&c.key))
                    .map(|c| format!("{} ({})", c.attribution, c.licence)),
            );
        }
        if !self.glo30.is_empty() {
            credits.push(self.credit.clone());
        }
        credits
    }
}
