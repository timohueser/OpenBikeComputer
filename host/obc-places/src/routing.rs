//! Routable source ways and their byte classification, before graph construction.

pub mod access;
pub mod country;
pub mod data;
mod device;
pub use device::{node_access, way_access, ProfileAccess};

/// Map a supported source highway to its device class. Motorways have no device class.
fn highway_class(highway: &str) -> Option<u8> {
    Some(match highway {
        "cycleway" | "cycleway_link" => 0,
        "path" | "path_link" => 1,
        "track" => 2,
        "footway" | "pedestrian" | "footway_link" => 3,
        "steps" => 4,
        "bridleway" | "bridleway_link" => 5,
        "living_street" | "living_street_link" => 6,
        "residential" => 7,
        "service" | "service_link" => 8,
        "unclassified" | "road" => 9,
        "tertiary" | "tertiary_link" => 10,
        "secondary" | "secondary_link" => 11,
        "primary" | "primary_link" => 12,
        // `trunk`/`trunk_link` are class 13; mode legality is checked separately.
        _ => return None,
    })
}

/// Map an OSM `surface=*` value to its surface class (3-bit). Absent or unrecognized gives `0`.
fn surface_class(surface: Option<&str>) -> u8 {
    match surface {
        Some("paved" | "asphalt" | "concrete" | "paving_stones" | "concrete:plates" | "concrete:lanes") => 1,
        Some("compacted" | "fine_gravel") => 2,
        Some("gravel" | "pebblestone" | "unpaved") => 3,
        Some("ground" | "dirt" | "earth") => 4,
        Some("sand" | "mud") => 5,
        Some("cobblestone" | "sett" | "unhewn_cobblestone") => 6,
        Some("grass" | "grass_paver") => 7,
        _ => 0,
    }
}

/// Classify a source highway and surface into the packed byte. Legality and direction are
/// separate in [`way_access`], so a mode-specific permission can override general restrictions.
pub fn classify<'a, I>(tags: I) -> Option<u8>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let (mut highway, mut surface) = (None, None);
    for (key, value) in tags {
        match key {
            "highway" => highway = Some(value),
            "surface" => surface = Some(value),
            _ => {}
        }
    }
    let class = match highway? {
        "trunk" | "trunk_link" => 13,
        other => highway_class(other)?,
    };
    Some((surface_class(surface) << 5) | class)
}

/// Whether a way has a supported source class. The importer must also check [`way_access`].
pub fn is_routable<'a, I>(tags: I) -> bool
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    classify(tags).is_some()
}

/// One routable way handed to the graph builder: the OSM node-id sequence, the matching µdeg
/// `(lon, lat)` coordinates in way order, and the way's packed [`classify`] `kind`. The caller
/// filters before pushing, so `kind` is always a real class here.
#[derive(Debug, Clone)]
pub struct RoutableWay {
    pub id: i64,
    pub node_ids: Vec<i64>,
    pub coords: Vec<(i32, i32)>,
    pub kind: u8,
    pub access: [ProfileAccess; 2],
}

impl RoutableWay {
    /// Explicitly open source topology for hand-built fixtures.
    pub fn new(id: i64, node_ids: Vec<i64>, coords: Vec<(i32, i32)>, kind: u8) -> Self {
        Self { id, node_ids, coords, kind, access: [ProfileAccess::OPEN; 2] }
    }
}
