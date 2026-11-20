//! The places of the route facts from the OSM objects of the extract: car parks, lifts, peaks,
//! huts with food, lakes and waterfalls; and the edges of the water that the water share measures.

use std::collections::{BTreeMap, HashMap};

use planner_router::shape::P;

use super::geometry::{around, bounds, inner_point, inside, ring_m2, segment_boxes, to_segment_m, widen, Grid, Line};
use super::{Kind, Lift, Place, Size};
use crate::extract::{Extract, Shape};
use crate::source::{Id, Tags};

/// A car park with this many spaces or more is large.
pub const LARGE_SPACES: u32 = 50;
/// A car park with no number of spaces and an area of this many square metres or more is large.
pub const LARGE_AREA_M2: f64 = 1_250.0;
/// A lake without a name needs this area.
pub const LAKE_MIN_M2: f64 = 10_000.0;
/// The car park of a lift is the nearest one within this distance of its valley station.
pub const VALLEY_PARKING_M: f64 = 300.0;

const LIFTS: [&str; 4] = ["cable_car", "gondola", "chair_lift", "mixed_lift"];
const FOOD: [&str; 3] = ["restaurant", "cafe", "biergarten"];
const BUILT: [&str; 3] = ["residential", "commercial", "retail"];

fn is(tags: &Tags, key: &str, values: &[&str]) -> bool {
    tags.get(key).is_some_and(|value| values.contains(&value))
}

/// The objects that the places and the water need, and the stations whose heights the lifts read.
pub fn wanted(tags: &Tags) -> bool {
    is(tags, "amenity", &["parking", "restaurant", "cafe", "biergarten"])
        || is(tags, "aerialway", &["cable_car", "gondola", "chair_lift", "mixed_lift", "station"])
        || is(tags, "natural", &["peak", "water"])
        || is(tags, "tourism", &["alpine_hut"])
        || is(tags, "waterway", &["river", "canal", "waterfall"])
        || is(tags, "landuse", &BUILT)
}

/// The water whose edge counts for the water share.
fn water(tags: &Tags) -> bool {
    (tags.contains("natural", "water") && is(tags, "water", &["lake", "pond", "reservoir", "river"]))
        || is(tags, "waterway", &["river", "canal"])
}

fn spaces(tags: &Tags) -> Option<u32> {
    tags.get("capacity").and_then(|value| value.trim().parse::<u32>().ok())
}

/// The size class of a car park: a whole number of spaces decides, else the area; a node without a
/// number is small.
pub fn size(tags: &Tags, area_m2: Option<f64>) -> Size {
    let large = match spaces(tags) {
        Some(spaces) => spaces >= LARGE_SPACES,
        None => area_m2.is_some_and(|area| area >= LARGE_AREA_M2),
    };
    if large {
        Size::Large
    } else {
        Size::Small
    }
}

/// The `ele` tag in metres: a number, with or without the unit `m`.
pub fn ele(tags: &Tags) -> Option<f64> {
    let value = tags.get("ele")?.trim();
    value.strip_suffix('m').unwrap_or(value).trim().parse::<f64>().ok().filter(|ele| ele.is_finite())
}

/// The top station and the valley station of a lift line: the end with the greater height, from the
/// `ele` tag of a station at the end, else from `height`. None when an end has no height or both
/// have the same.
pub fn stations(
    line: &[P],
    eles: &HashMap<P, f64>,
    height: &mut dyn FnMut(P) -> Result<Option<f64>, String>,
) -> Result<Option<(P, P)>, String> {
    let ends = [line[0], line[line.len() - 1]];
    let mut heights = [None; 2];
    for (end, height_m) in ends.iter().zip(&mut heights) {
        *height_m = match eles.get(end) {
            Some(&ele) => Some(ele),
            None => height(*end)?,
        };
    }
    Ok(match heights {
        [Some(a), Some(b)] if a > b => Some((ends[0], ends[1])),
        [Some(a), Some(b)] if b > a => Some((ends[1], ends[0])),
        _ => None,
    })
}

/// The places, by id, and the edges of the water.
pub struct Places {
    list: Vec<Place>,
    grid: Grid,
    water: Vec<[P; 2]>,
    water_grid: Grid,
    pub counts: Counts,
}

#[derive(Default)]
pub struct Counts {
    pub kinds: BTreeMap<&'static str, usize>,
    /// Lifts left out: an end has no height, or both ends have the same.
    pub lifts_without_height: usize,
    /// Car parks with an area and a whole number of spaces that the area calls large, and of them
    /// those with `LARGE_SPACES` or more.
    pub area_large_with_capacity: usize,
    pub of_them_50_or_more: usize,
}

impl Places {
    /// The places from the objects of `extract`. `height` gives the terrain height of a lift end.
    pub fn new(extract: &Extract, height: &mut dyn FnMut(P) -> Result<Option<f64>, String>) -> Result<Self, String> {
        let objects = &extract.objects;
        let rings = |shape: &Shape| match shape {
            Shape::Area(area) => area.outer.iter().chain(&area.inner).cloned().collect(),
            _ => Vec::new(),
        };
        let mut eles = HashMap::new();
        let mut built: Vec<Vec<Vec<P>>> = Vec::new();
        for object in objects {
            match &object.shape {
                Shape::Point(p) if object.tags.contains("aerialway", "station") => {
                    eles.extend(ele(&object.tags).map(|ele| (*p, ele)));
                }
                Shape::Area(_) if is(&object.tags, "landuse", &BUILT) => built.push(rings(&object.shape)),
                _ => {}
            }
        }
        let built_grid =
            Grid::new(1_000.0, built.iter().enumerate().map(|(i, rings)| (bounds(&rings.concat()), i as u32)));
        let in_built = |p: P| built_grid.items([[p, p]]).iter().any(|&i| inside(p, &built[i as usize]));
        let mut counts = Counts::default();
        let mut list = Vec::new();
        for object in objects {
            let tags = &object.tags;
            let shape = &object.shape;
            let kind = if tags.contains("amenity", "parking") {
                Kind::Parking
            } else if is(tags, "aerialway", &LIFTS) {
                Kind::Lift
            } else if tags.contains("natural", "peak") {
                Kind::Peak
            } else if tags.contains("tourism", "alpine_hut") || is(tags, "amenity", &FOOD) {
                Kind::Hut
            } else if tags.contains("natural", "water") && is(tags, "water", &["lake", "pond"]) {
                Kind::Lake
            } else if tags.contains("waterway", "waterfall") {
                Kind::Waterfall
            } else {
                continue;
            };
            let area_m2 = match shape {
                Shape::Area(area) => {
                    let sum = |rings: &[Vec<P>]| rings.iter().map(|ring| ring_m2(ring)).sum::<f64>();
                    Some(sum(&area.outer) - sum(&area.inner))
                }
                _ => None,
            };
            let at = match shape {
                Shape::Point(p) => *p,
                Shape::Line(line) => line[0],
                Shape::Area(area) => inner_point(&area.outer),
            };
            let mut place = Place {
                id: object.id,
                kind,
                name: tags.get("name").map(str::to_string),
                ele_m: ele(tags).map(|ele| ele.round() as i32),
                at,
                size: None,
                lift: None,
                rings: rings(shape),
            };
            let point = matches!(shape, Shape::Point(_));
            let keep = match kind {
                Kind::Parking => {
                    place.size = Some(size(tags, area_m2));
                    let keep = (point || area_m2.is_some()) && !is(tags, "access", &["private", "customers"]);
                    if let (true, Some(spaces), Some(area)) = (keep, spaces(tags), area_m2) {
                        if area >= LARGE_AREA_M2 {
                            counts.area_large_with_capacity += 1;
                            counts.of_them_50_or_more += usize::from(spaces >= LARGE_SPACES);
                        }
                    }
                    keep
                }
                Kind::Lift => match shape {
                    Shape::Line(line) => match stations(line, &eles, height)? {
                        Some((top, valley)) => {
                            place.at = top;
                            place.lift = Some(Lift { valley, parking: None });
                            true
                        }
                        None => {
                            counts.lifts_without_height += 1;
                            false
                        }
                    },
                    _ => false,
                },
                Kind::Peak | Kind::Waterfall => point,
                Kind::Hut => (point || area_m2.is_some()) && (tags.contains("tourism", "alpine_hut") || !in_built(at)),
                Kind::Lake => tags.get("name").is_some() || area_m2.is_some_and(|area| area >= LAKE_MIN_M2),
            };
            if keep {
                *counts.kinds.entry(kind.name()).or_default() += 1;
                list.push(place);
            }
        }
        let grid = Grid::new(1_000.0, list.iter().enumerate().map(|(i, place)| (place_bounds(place), i as u32)));
        let mut places = Places { list, grid, water: Vec::new(), water_grid: Grid::new(250.0, []), counts };
        let parking: Vec<Option<Id>> = places.list.iter().map(|place| places.valley_parking(place)).collect();
        for (place, parking) in places.list.iter_mut().zip(parking) {
            if let Some(lift) = &mut place.lift {
                lift.parking = parking;
            }
        }
        for object in objects.iter().filter(|object| water(&object.tags)) {
            let lines = match &object.shape {
                Shape::Line(line) => vec![line.clone()],
                Shape::Area(_) => rings(&object.shape),
                Shape::Point(_) => Vec::new(),
            };
            places.water.extend(lines.iter().flat_map(|line| line.windows(2).map(|w| [w[0], w[1]])));
        }
        let edges = places.water.iter().enumerate().map(|(i, edge)| (bounds(edge), i as u32));
        places.water_grid = Grid::new(250.0, edges);
        Ok(places)
    }

    /// The nearest car park within `VALLEY_PARKING_M` of the valley station of a lift.
    fn valley_parking(&self, lift: &Place) -> Option<Id> {
        let valley = lift.lift.as_ref()?.valley;
        let line = Line::new(&[valley]);
        let parks = self.near(&[valley], VALLEY_PARKING_M).filter(|place| place.kind == Kind::Parking);
        let distances = parks.map(|park| (park.distance(&line, VALLEY_PARKING_M), park.id));
        let near = distances.filter(|(distance, _)| *distance <= VALLEY_PARKING_M);
        near.min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))).map(|(_, id)| id)
    }

    /// The place of `id`.
    pub fn get(&self, id: Id) -> Option<&Place> {
        self.list.binary_search_by_key(&id, |place| place.id).ok().map(|at| &self.list[at])
    }

    /// The places whose boxes may lie within `reach_m` of `line`, by id.
    pub fn near(&self, line: &[P], reach_m: f64) -> impl Iterator<Item = &Place> {
        let boxes: Vec<[P; 2]> = match line.len() {
            1 => vec![around(line, reach_m)],
            _ => segment_boxes(line).map(|segment| widen(segment, reach_m)).collect(),
        };
        self.grid.items(boxes).into_iter().map(|i| &self.list[i as usize])
    }

    /// Whether `p` lies within `reach_m` of the edge of water.
    pub fn by_water(&self, p: P, reach_m: f64) -> bool {
        let edges = self.water_grid.items([around(&[p], reach_m)]);
        edges.into_iter().any(|i| to_segment_m(p, self.water[i as usize]) <= reach_m)
    }
}

/// The box of a place: of its rings, else of its point.
fn place_bounds(place: &Place) -> [P; 2] {
    match place.rings.is_empty() {
        true => bounds(&[place.at]),
        false => bounds(&place.rings.concat()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::extract::{Area, Object};

    /// The places of objects with ways ids from 1, with no terrain.
    pub(crate) fn places(objects: Vec<(&[(&str, &str)], Shape)>) -> Places {
        let objects = objects.into_iter().enumerate();
        let objects = objects
            .map(|(i, (tags, shape))| Object {
                id: Id::Way(i as i64 + 1),
                tags: Tags::from_pairs(tags.iter().copied()),
                shape,
            })
            .collect();
        Places::new(&Extract { objects, incomplete: 0 }, &mut |_| Ok(None)).unwrap()
    }

    /// A square area of `side` microdegrees from `corner`.
    pub(crate) fn square(corner: P, side: i32) -> Shape {
        let [x, y] = corner;
        let ring = vec![[x, y], [x, y + side], [x + side, y + side], [x + side, y], [x, y]];
        Shape::Area(Area { outer: vec![ring], inner: Vec::new() })
    }

    #[test]
    fn a_car_park_takes_its_class_from_capacity_then_area_and_a_node_is_small() {
        let parking = |capacity: &str| Tags::from_pairs([("amenity", "parking"), ("capacity", capacity)]);
        assert_eq!(size(&parking("60"), Some(100.0)), Size::Large);
        assert_eq!(size(&parking("12"), Some(5_000.0)), Size::Small);
        assert_eq!(size(&parking("ca. 50"), Some(1_300.0)), Size::Large);
        assert_eq!(size(&parking("ca. 50"), Some(1_200.0)), Size::Small);
        // At 47° N, 500 by 500 microdegrees are about 2,100 m².
        let places = places(vec![
            (&[("amenity", "parking")], Shape::Point([8_000_000, 47_000_000])),
            (&[("amenity", "parking")], square([8_010_000, 47_000_000], 500)),
            (&[("amenity", "parking"), ("capacity", "12")], square([8_020_000, 47_000_000], 500)),
            (&[("amenity", "parking"), ("access", "private")], Shape::Point([8_030_000, 47_000_000])),
            (&[("amenity", "parking"), ("access", "customers")], Shape::Point([8_040_000, 47_000_000])),
        ]);
        let sizes: Vec<(Id, Option<Size>)> = places.list.iter().map(|place| (place.id, place.size)).collect();
        let large = Some(Size::Large);
        let small = Some(Size::Small);
        assert_eq!(sizes, [(Id::Way(1), small), (Id::Way(2), large), (Id::Way(3), small)]);
        assert_eq!((places.counts.area_large_with_capacity, places.counts.of_them_50_or_more), (1, 0));
    }

    #[test]
    fn the_top_station_is_the_higher_end_by_ele_then_by_terrain() {
        let (valley, top) = ([8_000_000, 47_000_000], [8_010_000, 47_010_000]);
        let line = [valley, top];
        let terrain = |p: P| if p == valley { Some(900.0) } else { Some(800.0) };
        let mut height = |p: P| -> Result<Option<f64>, String> { Ok(terrain(p)) };
        // By terrain alone, the first end is higher; the `ele` of the second end wins over it.
        assert_eq!(stations(&line, &HashMap::new(), &mut height).unwrap(), Some((valley, top)));
        let eles = HashMap::from([(top, 2_000.0)]);
        assert_eq!(stations(&line, &eles, &mut height).unwrap(), Some((top, valley)));
        let mut none = |p: P| -> Result<Option<f64>, String> { Ok((p == valley).then_some(900.0)) };
        assert_eq!(stations(&line, &HashMap::new(), &mut none).unwrap(), None);
        let mut flat = |_: P| -> Result<Option<f64>, String> { Ok(Some(900.0)) };
        assert_eq!(stations(&line, &HashMap::new(), &mut flat).unwrap(), None);
    }

    #[test]
    fn the_place_rules_follow_the_tags() {
        // A residential area of about 760 m square, a restaurant in it and one outside it, a hut in
        // it, an unnamed pond of about 0.9 ha and a named one, and a peak as a node and as an area.
        let town = square([8_000_000, 47_000_000], 10_000);
        let pond = square([8_100_000, 47_000_000], 1_050);
        let places = places(vec![
            (&[("landuse", "residential")], town),
            (&[("amenity", "restaurant")], Shape::Point([8_005_000, 47_005_000])),
            (&[("amenity", "cafe")], Shape::Point([8_050_000, 47_005_000])),
            (&[("tourism", "alpine_hut")], Shape::Point([8_005_000, 47_006_000])),
            (&[("natural", "water"), ("water", "pond")], pond.clone()),
            (&[("natural", "water"), ("water", "pond"), ("name", "Seeli")], pond),
            (&[("natural", "peak"), ("ele", "2128 m")], Shape::Point([8_200_000, 47_000_000])),
            (&[("natural", "peak")], square([8_300_000, 47_000_000], 100)),
        ]);
        let kept: Vec<(Id, Kind)> = places.list.iter().map(|place| (place.id, place.kind)).collect();
        let way = Id::Way;
        assert_eq!(kept, [(way(3), Kind::Hut), (way(4), Kind::Hut), (way(6), Kind::Lake), (way(7), Kind::Peak)]);
        assert_eq!(places.get(way(7)).unwrap().ele_m, Some(2128));
        // The edge of the named pond is water; its middle is about 40 m from the edge.
        assert!(places.by_water([8_100_000, 47_000_500], 100.0));
        assert!(!places.by_water([8_100_000 + 525, 47_000_525], 30.0));
    }
}
