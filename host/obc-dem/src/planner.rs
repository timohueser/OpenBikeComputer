//! The heights of the planner: its terrain tiles and the slopes of its routes. The reference
//! archive gives the height where it holds the ground, and GLO-30 elsewhere.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use obc_data::store::sha256_hex;

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
pub fn tiles_in(dir: &Path, bounds: [f64; 4]) -> Vec<PathBuf> {
    tiles(bounds).iter().map(|tile| dir.join(tile.file_name())).filter(|path| path.is_file()).collect()
}

pub struct Terrain {
    fallback: DemMosaic,
    reference: Option<ReferenceArchive>,
    cache: VecDeque<((u32, u32), Option<ReferenceTile>)>,
    /// The SHA-256 of each GLO-30 file, then of each reference tile that `bounds` reaches.
    pub identities: Vec<String>,
    used: BTreeSet<String>,
}

impl Terrain {
    /// The GLO-30 files `glo30`, in the order of [`tiles`], and the reference archive at
    /// `reference`, for heights in `bounds`.
    pub fn open(glo30: &[PathBuf], reference: Option<&Path>, bounds: [f64; 4]) -> Result<Self, String> {
        let mut fallback = DemMosaic::default();
        let mut identities = Vec::new();
        for path in glo30 {
            identities.push(sha256_hex(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?));
            fallback.push(DemTile::open(path)?);
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
        Ok(Self { fallback, reference, cache: VecDeque::new(), identities, used: BTreeSet::new() })
    }

    /// The height in metres at `lat`, `lon` in microdegrees, or `None` where no source has one.
    pub fn height(&mut self, lat: i32, lon: i32) -> Result<Option<f64>, String> {
        let mut best = None;
        if let Some(archive) = &self.reference {
            let window = Window {
                lat_lo: lat as i64 - 32,
                lat_hi: lat as i64 + 32,
                lon_lo: lon as i64 - 32,
                lon_hi: lon as i64 + 32,
            };
            for key in window.tiles() {
                let tile = if let Some(index) = self.cache.iter().position(|(id, _)| id == &key) {
                    self.cache.remove(index).unwrap().1
                } else {
                    match archive.tile(key.0, key.1)? {
                        TileLookup::Held { tile, sources } => {
                            self.used.extend(sources.iter().cloned());
                            Some(tile)
                        }
                        TileLookup::Absent | TileLookup::Unknown => None,
                    }
                };
                if let Some(tile) = &tile {
                    tile.centres_in(window, |_, _, metres| best = Some(metres as f64));
                }
                self.cache.push_back((key, tile));
                if self.cache.len() > 32 {
                    self.cache.pop_front();
                }
            }
        }
        Ok(best.or_else(|| self.fallback.height(lat as f64 * 1e-6, lon as f64 * 1e-6)))
    }

    /// The credits of the sources that gave a height so far.
    pub fn attribution(&self) -> Vec<String> {
        let mut credits = Vec::new();
        if let Some(archive) = &self.reference {
            credits.extend(
                archive
                    .credits()
                    .iter()
                    .filter(|c| self.used.contains(&c.key))
                    .map(|c| format!("{} ({})", c.attribution, c.licence)),
            );
        }
        if !self.fallback.is_empty() {
            credits.push(obc_data::sources::attribution(crate::step::GLO30).into());
        }
        credits
    }
}
