//! Optional adapter to the repository's terrain readers. The routing crates need no OBC map format.
use obc_dem::{
    geotiff::{DemMosaic, DemTile},
    reference::{ReferenceArchive, ReferenceTile, TileLookup, Window},
};
use route_engine::{model::Point, package::digest};
use std::{
    collections::{BTreeSet, VecDeque},
    path::Path,
};

pub struct Terrain {
    fallback: DemMosaic,
    reference: Option<ReferenceArchive>,
    cache: VecDeque<((u32, u32), Option<ReferenceTile>)>,
    pub identities: Vec<String>,
    used: BTreeSet<String>,
}

impl Terrain {
    pub fn open(dem: Option<&Path>, reference: Option<&Path>, bounds: [f64; 4]) -> Result<Self, String> {
        let mut fallback = DemMosaic::default();
        let mut identities = Vec::new();
        if let Some(directory) = dem {
            for lat in bounds[1].floor() as i32..=bounds[3].floor() as i32 {
                for lon in bounds[0].floor() as i32..=bounds[2].floor() as i32 {
                    let path = directory.join(format!(
                        "Copernicus_DSM_COG_10_{}{:02}_00_{}{:03}_00_DEM.tif",
                        if lat >= 0 { 'N' } else { 'S' },
                        lat.abs(),
                        if lon >= 0 { 'E' } else { 'W' },
                        lon.abs()
                    ));
                    if path.is_file() {
                        identities.push(digest(&std::fs::read(&path).map_err(|e| e.to_string())?));
                        fallback.push(DemTile::open(&path)?);
                    }
                }
            }
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

    pub fn height(&mut self, point: Point) -> Result<Option<f64>, String> {
        let mut best = None;
        if let Some(archive) = &self.reference {
            let window = Window {
                lat_lo: point.lat as i64 - 32,
                lat_hi: point.lat as i64 + 32,
                lon_lo: point.lon as i64 - 32,
                lon_hi: point.lon as i64 + 32,
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
        Ok(best.or_else(|| self.fallback.height(point.lat as f64 * 1e-6, point.lon as f64 * 1e-6)))
    }

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
            credits.push(obc_data::sources::attribution("copernicus-glo-30").into());
        }
        credits
    }
}
