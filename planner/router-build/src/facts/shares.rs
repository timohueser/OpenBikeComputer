//! The shares of the length of a route, from its samples: the points of its line at each 100 m, as
//! `profile_m` has them. A sample is paved, near a big road, by water, or in forest.

use std::collections::BTreeMap;

use obc_data::pmtiles::Archive;
use planner_router::geometry::{at, cumulative};
use planner_router::model::{Point, Surface};
use planner_router::shape::P;
use rayon::prelude::*;

use super::geometry::{around, bounds, meets, to_segment_m, Grid};
use super::places::Places;
use crate::country::Country;
use crate::osm::surface_class;
use crate::source::{highway_access, Data, NodeRef, Tags};

pub const SAMPLE_M: f64 = 100.0;
/// A sample takes the surface of the nearest road or path within this distance.
pub const PAVED_M: f64 = 15.0;
pub const BIG_ROAD_M: f64 = 200.0;
/// The distance to the edge of water.
pub const WATER_M: f64 = 100.0;
pub const CANOPY_ZOOM: u8 = 12;
/// The share of the samples that need a canopy value for a canopy share.
pub const CANOPY_SHARE: f64 = 0.9;
/// A canopy cell with a cover of this many tenths or more is forest.
pub const FOREST_COVER: u16 = 5;

const BIG_ROADS: [&str; 3] = ["motorway", "trunk", "primary"];
/// The roads that are paved when they have no `surface` tag.
const PAVED_ROADS: [&str; 12] = [
    "motorway",
    "motorway_link",
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
    "unclassified",
    "residential",
];

/// The points of `line` at each 100 m of `length_m`, with the line scaled to `length_m`, and its
/// end: one for each entry of `profile_m`.
pub fn samples(line: &[P], length_m: f64) -> Vec<P> {
    let points: Vec<Point> = line.iter().map(|p| Point { lon: p[0], lat: p[1], elevation: 0.0 }).collect();
    let along = cumulative(&points);
    let total = along[along.len() - 1];
    let scale = if length_m > 0.0 { total / length_m } else { 0.0 };
    let count = (length_m / SAMPLE_M).floor() as usize + 2;
    let sample = |i: usize| {
        let p = at(&points, &along, (i as f64 * SAMPLE_M * scale).min(total));
        [p.lon, p.lat]
    };
    (0..count - 1).map(sample).chain([line[line.len() - 1]]).collect()
}

/// How a sample is paved: by its nearest road or path within `PAVED_M`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paved {
    No,
    /// Its `surface` is a paved surface of the router.
    Tagged,
    /// It has no `surface`, and it is a road from `motorway` to `residential`.
    Untagged,
}

/// The roads and paths of the routing source: the ways with a road class that are no areas, and
/// the big roads, those not in a tunnel.
pub struct Roads<'a> {
    osm: &'a Data,
    ways: Grid,
    big: Grid,
}

/// The boxes of the segments of a way whose ends the source holds.
fn way_boxes(points: &[NodeRef]) -> impl Iterator<Item = [P; 2]> + '_ {
    let present = points.windows(2).filter(|w| w[0].present() && w[1].present());
    present.map(|w| bounds(&[[w[0].lon, w[0].lat], [w[1].lon, w[1].lat]]))
}

/// The distance from `p` to the nearest segment of a way whose ends the source holds, when that is
/// at most `reach_m`.
fn way_distance(p: P, points: &[NodeRef], reach_m: f64) -> Option<f64> {
    let near = around(&[p], reach_m);
    let segments = points.windows(2).filter(|w| w[0].present() && w[1].present());
    let segments = segments.map(|w| [[w[0].lon, w[0].lat], [w[1].lon, w[1].lat]]);
    let segments = segments.filter(|segment| meets(near, bounds(segment)));
    segments.map(|segment| to_segment_m(p, segment)).filter(|d| *d <= reach_m).min_by(f64::total_cmp)
}

/// The road class of a way with a class of the router that is no area.
fn road<'t>(tags: &'t Tags) -> Option<&'t str> {
    let highway = tags.get("highway")?;
    let known = highway_access(highway, Country::default()).is_some();
    (known && !tags.contains("area", "yes")).then_some(highway)
}

fn big_way(tags: &Tags) -> bool {
    road(tags).is_some_and(|highway| BIG_ROADS.contains(&highway))
        && tags.get("tunnel").is_none_or(|value| value == "no")
}

impl<'a> Roads<'a> {
    pub fn new(osm: &'a Data) -> Self {
        let rows = |keep: fn(&Tags) -> bool| {
            let rows = osm.ways.iter().enumerate().filter(move |(_, way)| keep(&way.tags));
            rows.flat_map(|(row, way)| way_boxes(way.points).map(move |bounds| (bounds, row as u32)))
        };
        let ways = Grid::new(250.0, rows(|tags| road(tags).is_some()));
        Roads { osm, ways, big: Grid::new(1_000.0, rows(big_way)) }
    }

    pub fn paved(&self, p: P) -> Paved {
        let mut nearest: Option<(f64, u32)> = None;
        for row in self.ways.items([around(&[p], PAVED_M)]) {
            let way = self.osm.ways.row(row as usize);
            if let Some(distance) = way_distance(p, way.points, PAVED_M) {
                if nearest.is_none_or(|(best, _)| distance < best) {
                    nearest = Some((distance, row));
                }
            }
        }
        let Some((_, row)) = nearest else { return Paved::No };
        let tags = self.osm.ways.row(row as usize).tags;
        match tags.get("surface") {
            Some(surface) if surface_class(surface) == Surface::Paved => Paved::Tagged,
            Some(_) => Paved::No,
            None if tags.get("highway").is_some_and(|highway| PAVED_ROADS.contains(&highway)) => Paved::Untagged,
            None => Paved::No,
        }
    }

    pub fn big_road(&self, p: P) -> bool {
        let rows = self.big.items([around(&[p], BIG_ROAD_M)]);
        rows.into_iter().any(|row| way_distance(p, self.osm.ways.row(row as usize).points, BIG_ROAD_M).is_some())
    }
}

/// Forest of a canopy code (`specs/trailview-tiles.md`, section Canopy): None for an unknown cell.
fn forest_code(code: u16) -> Option<bool> {
    match code {
        u16::MAX => None,
        0 => Some(false),
        code => Some((code >> 7) & 15 >= FOREST_COVER),
    }
}

/// The codes of a canopy tile, row after row.
fn decode(bytes: &[u8]) -> Result<Vec<u16>, String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP).map_err(|e| e.to_string())?;
    let rgb = image.to_rgb8();
    if rgb.dimensions() != (256, 256) {
        return Err(format!("a canopy tile is {:?} cells, not 256 × 256", rgb.dimensions()));
    }
    Ok(rgb.pixels().map(|pixel| u16::from(pixel.0[0]) | u16::from(pixel.0[1]) << 8).collect())
}

/// Whether each point lies in forest, from the canopy archive; None in an unknown cell and in a
/// tile that the archive lacks. Each tile decodes once.
pub fn forest(archive: &Archive, points: &[P]) -> Result<Vec<Option<bool>>, String> {
    let mut tiles: BTreeMap<(u32, u32), Vec<(usize, usize)>> = BTreeMap::new();
    let last = (1u32 << CANOPY_ZOOM) - 1;
    for (i, p) in points.iter().enumerate() {
        let (x, y) = crate::grid::mercator(p[0] as f64 * 1e-6, p[1] as f64 * 1e-6, CANOPY_ZOOM.into());
        let (column, row) = ((x.max(0.0) as u32).min(last), (y.max(0.0) as u32).min(last));
        let cell = |value: f64, tile: u32| ((value - f64::from(tile)) * 256.0).clamp(0.0, 255.0) as usize;
        tiles.entry((column, row)).or_default().push((i, cell(y, row) * 256 + cell(x, column)));
    }
    let tiles: Vec<_> = tiles.into_iter().collect();
    let values = tiles.par_iter().map(|((x, y), cells)| -> Result<Vec<(usize, Option<bool>)>, String> {
        let Some(bytes) = archive.get(CANOPY_ZOOM, *x, *y)? else { return Ok(Vec::new()) };
        let codes = decode(&bytes)?;
        Ok(cells.iter().map(|&(i, cell)| (i, forest_code(codes[cell]))).collect())
    });
    let mut forest = vec![None; points.len()];
    for (i, value) in values.collect::<Result<Vec<_>, String>>()?.into_iter().flatten() {
        forest[i] = value;
    }
    Ok(forest)
}

/// The counts of the samples of a route.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shares {
    pub samples: u32,
    pub paved: u32,
    /// The paved samples on roads without `surface`.
    pub untagged: u32,
    pub big_road: u32,
    pub water: u32,
    /// The forest samples and the samples with a canopy value; None without a canopy archive.
    pub canopy: Option<(u32, u32)>,
}

/// `part` of `whole` in whole percent.
pub fn percent(part: u32, whole: u32) -> u8 {
    (f64::from(part) * 100.0 / f64::from(whole.max(1))).round() as u8
}

impl Shares {
    /// The shares of `samples`, with the forest of each sample when the release has a canopy.
    pub fn new(samples: &[P], roads: &Roads, places: &Places, forest: Option<&[Option<bool>]>) -> Self {
        let mut shares = Shares { samples: samples.len() as u32, ..Shares::default() };
        for &p in samples {
            match roads.paved(p) {
                Paved::Tagged => shares.paved += 1,
                Paved::Untagged => (shares.paved, shares.untagged) = (shares.paved + 1, shares.untagged + 1),
                Paved::No => {}
            }
            shares.big_road += u32::from(roads.big_road(p));
            shares.water += u32::from(places.by_water(p, WATER_M));
        }
        shares.canopy = forest.map(|forest| {
            let known = forest.iter().flatten();
            (known.clone().filter(|&&forest| forest).count() as u32, known.count() as u32)
        });
        shares
    }

    /// The forest share, when `CANOPY_SHARE` of the samples or more have a canopy value.
    pub fn canopy_pct(&self) -> Option<u8> {
        let (forest, known) = self.canopy?;
        (f64::from(known) >= CANOPY_SHARE * f64::from(self.samples)).then(|| percent(forest, known))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::facts::places::tests::places;
    use crate::source::{Builder, Ways};

    /// The tags and the line of a way.
    type WayRow<'a> = (&'a [(&'a str, &'a str)], &'a [P]);

    /// The routing source of the ways, each node present.
    fn data(ways: &[WayRow]) -> Data {
        let mut builder = Builder::<Ways>::default();
        let mut points = Vec::new();
        for (i, (tags, line)) in ways.iter().enumerate() {
            let nodes: Vec<i64> = (0..line.len() as i64).map(|k| i as i64 * 100 + k).collect();
            builder.push(i as i64 + 1, &nodes, &Tags::from_pairs(tags.iter().copied()));
            points.extend(line.iter().map(|p| NodeRef {
                lat: p[1],
                lon: p[0],
                flags: NodeRef::PRESENT,
                ..NodeRef::missing(false)
            }));
        }
        let (mut ways, _) = builder.finish();
        ways.set_points(points.into_iter().collect());
        Data { ways, ..Data::default() }
    }

    /// A line from south to north `metres` east of `lon`, at 47° N.
    fn north(metres: f64) -> [P; 2] {
        let lon = 8_000_000 + (metres / (47f64.to_radians().cos() * 0.111195)).round() as i32;
        [[lon, 46_990_000], [lon, 47_010_000]]
    }

    #[test]
    fn paved_takes_the_nearest_highway_within_15_m() {
        let p = [8_000_000, 47_000_000];
        let path: &[(&str, &str)] = &[("highway", "path"), ("surface", "gravel")];
        let road: &[(&str, &str)] = &[("highway", "residential")];
        let paved = |ways: &[WayRow]| Roads::new(&data(ways)).paved(p);
        assert_eq!(paved(&[(path, &north(8.0)), (road, &north(-12.0))]), Paved::No);
        assert_eq!(paved(&[(road, &north(-12.0))]), Paved::Untagged);
        let asphalt: &[(&str, &str)] = &[("highway", "track"), ("surface", "asphalt")];
        assert_eq!(paved(&[(asphalt, &north(10.0))]), Paved::Tagged);
        assert_eq!(paved(&[(asphalt, &north(20.0))]), Paved::No);
        let square: &[(&str, &str)] = &[("highway", "pedestrian"), ("area", "yes"), ("surface", "asphalt")];
        assert_eq!(paved(&[(square, &north(5.0))]), Paved::No);
    }

    #[test]
    fn big_road_and_water_use_their_distances() {
        let p = [8_000_000, 47_000_000];
        let primary: &[(&str, &str)] = &[("highway", "primary")];
        let tunnel: &[(&str, &str)] = &[("highway", "motorway"), ("tunnel", "yes")];
        let big = |ways: &[WayRow]| Roads::new(&data(ways)).big_road(p);
        assert!(big(&[(primary, &north(190.0))]));
        assert!(!big(&[(primary, &north(210.0))]));
        assert!(!big(&[(tunnel, &north(50.0))]));
        let river =
            |metres: f64| places(vec![(&[("waterway", "river")], crate::extract::Shape::Line(north(metres).to_vec()))]);
        assert!(river(95.0).by_water(p, WATER_M));
        assert!(!river(105.0).by_water(p, WATER_M));
    }

    #[test]
    fn canopy_needs_values_at_90_percent_of_samples() {
        // Tile 12/2143/1443 holds Engelberg. Its first four cells of row 0 are unknown, open, forest
        // with a cover of 5 and forest with a cover of 4; the tile east of it is missing.
        let (x, y) = (2143, 1443);
        let codes = [u16::MAX, 0, 5 << 7 | 20, 4 << 7 | 20];
        let mut rgb = vec![0u8; 256 * 256 * 3];
        for (cell, code) in codes.iter().enumerate() {
            rgb[cell * 3..cell * 3 + 2].copy_from_slice(&code.to_le_bytes());
        }
        let mut tile = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut tile)
            .encode(&rgb, 256, 256, image::ExtendedColorType::Rgb8)
            .unwrap();
        let path = std::env::temp_dir().join(format!("obc-canopy-{}.pmtiles", std::process::id()));
        std::fs::write(&path, archive(&[(obc_data::pmtiles::tile_id(CANOPY_ZOOM, x, y), tile)], 1, "{}")).unwrap();
        let archive = Archive::open(&path).unwrap();
        let n = f64::from(1u32 << CANOPY_ZOOM);
        let centre = |column: f64, row: f64| {
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * row / n)).sinh().atan().to_degrees();
            [((column / n * 360.0 - 180.0) * 1e6).round() as i32, (lat * 1e6).round() as i32]
        };
        let points: Vec<P> =
            (0..4).map(|cell| centre(f64::from(x) + (cell as f64 + 0.5) / 256.0, f64::from(y) + 0.5 / 256.0)).collect();
        let east = centre(f64::from(x) + 1.5, f64::from(y) + 0.5);
        let forest = forest(&archive, &[points.clone(), vec![east]].concat()).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(forest, [None, Some(false), Some(true), Some(false), None]);
        let shares = |known: u32| Shares { samples: 10, canopy: Some((3, known)), ..Shares::default() };
        assert_eq!((shares(9).canopy_pct(), shares(8).canopy_pct()), (Some(33), None));
    }

    /// A PMTiles archive of `tiles`, sorted by id, as stored, with `metadata`.
    pub(crate) fn archive(tiles: &[(u64, Vec<u8>)], tile_compression: u8, metadata: &str) -> Vec<u8> {
        let mut values = vec![tiles.len() as u64];
        values.extend(tiles.iter().scan(0, |last, (id, _)| Some(id - std::mem::replace(last, *id))));
        values.extend(tiles.iter().map(|_| 1));
        values.extend(tiles.iter().map(|(_, tile)| tile.len() as u64));
        values.extend(tiles.iter().enumerate().map(|(i, _)| u64::from(i == 0)));
        let mut root = Vec::new();
        for mut value in values {
            while value >= 0x80 {
                root.push(value as u8 | 0x80);
                value >>= 7;
            }
            root.push(value as u8);
        }
        let metadata_at = 127 + root.len() as u64;
        let data_at = metadata_at + metadata.len() as u64;
        let data: Vec<u8> = tiles.iter().flat_map(|(_, tile)| tile.iter().copied()).collect();
        let mut file = b"PMTiles\x03".to_vec();
        for word in [127, root.len() as u64, metadata_at, metadata.len() as u64, data_at, 0, data_at, data.len() as u64]
        {
            file.extend(word.to_le_bytes());
        }
        file.resize(97, 0);
        file.extend([1, tile_compression]);
        file.resize(127, 0);
        file.extend(root);
        file.extend(metadata.as_bytes());
        file.extend(data);
        file
    }
}
