//! Routable source ways and their byte classification, before graph construction.

/// Map an OSM `highway=*` value to its highway class (5-bit). `None` for a value that carries no
/// class, including `motorway`, which is always bike-illegal, and `trunk`, which [`classify`]
/// handles separately (class 13, only with `bicycle=yes`).
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
        // `trunk`/`trunk_link` are class 13, but only with `bicycle=yes` — see `classify`.
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

/// Classify a way into its packed way-kind byte, or `None` if the way is not routable.
///
/// The byte is `kind = (surface_class << 5) | highway_class`, from the canonical [`highway_class`]
/// and [`surface_class`] tables that `OBCM_Spec.md` mirrors. The device never sees raw tags: a
/// routing profile weights edges purely off this byte.
///
/// A way is not routable when any hard-exclude applies: `highway=motorway|motorway_link`;
/// `highway=trunk|trunk_link` unless `bicycle=yes`; `motorroad=yes`; `bicycle=no|use_sidepath`;
/// `access=no|private`. Everything else is kept, including `footway` and `steps`, which are legal to
/// walk a bike along, and `bicycle=dismount`. Preference rather than legality is the router's job.
pub fn classify<'a, I>(tags: I) -> Option<u8>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut highway: Option<&str> = None;
    let mut surface: Option<&str> = None;
    let mut bicycle: Option<&str> = None;
    let mut access: Option<&str> = None;
    let mut motorroad: Option<&str> = None;
    for (k, v) in tags {
        match k {
            "highway" => highway = Some(v),
            "surface" => surface = Some(v),
            "bicycle" => bicycle = Some(v),
            "access" => access = Some(v),
            "motorroad" => motorroad = Some(v),
            _ => {}
        }
    }

    // Hard bike-illegal excludes (checked before any class assignment).
    if matches!(access, Some("no") | Some("private")) {
        return None;
    }
    if motorroad == Some("yes") {
        return None;
    }
    if matches!(bicycle, Some("no") | Some("use_sidepath")) {
        return None;
    }

    let highway = highway?;
    let hclass = match highway {
        // `trunk` is legal for bikes only when explicitly allowed, and is then its own class.
        "trunk" | "trunk_link" => {
            if bicycle == Some("yes") {
                13
            } else {
                return None;
            }
        }
        other => highway_class(other)?,
    };
    Some((surface_class(surface) << 5) | hclass)
}

/// Whether a way is routable for a bike: exactly `classify(tags).is_some()`. A named predicate
/// because that is how the ingest reads it — routability first, then the kind.
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
    pub node_ids: Vec<i64>,
    pub coords: Vec<(i32, i32)>,
    pub kind: u8,
}
