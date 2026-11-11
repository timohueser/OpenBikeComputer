//! The source OSM model and the tag rules that the import, the overlay index and the route
//! catalog share. No source object enters the routing package.
use crate::column::{Column, Layout, Lists};
use crate::country::Country;
use crate::sort;
use planner_router::closures::{Closure, Kind, Set};
use planner_router::model::{Point, Surface, BIKE, FOOT, NO_ELEVATION, PUSH};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

/// What an access value tells the router. Only `no` blocks a mode; every other restriction stays
/// routable, and the route reports it as a closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Open,
    Uncertain(Kind),
    Closed,
}

/// A list such as `agricultural;forestry` takes its most open value.
pub fn classify(value: &str) -> Access {
    let one = |value: &str| match value.trim() {
        // `mtb` designates a way for mountain bikes, as in Switzerland.
        "yes" | "designated" | "official" | "permissive" | "mtb" | "optional_sidepath" => Access::Open,
        // `dismount` closes riding only; pushing follows foot access.
        "no" | "dismount" => Access::Closed,
        "permit" => Access::Uncertain(Kind::Permit),
        "private" => Access::Uncertain(Kind::Private),
        "agricultural" | "forestry" => Access::Uncertain(Kind::Farm),
        "use_sidepath" => Access::Uncertain(Kind::Sidepath),
        "discouraged" => Access::Uncertain(Kind::Discouraged),
        "destination" | "customers" | "delivery" | "residents" | "military" | "psv" | "bus" | "emergency" | "hgv"
        | "taxi" | "motor_vehicle" | "motorcar" => Access::Uncertain(Kind::Limited),
        _ => Access::Uncertain(Kind::Unclear),
    };
    value.split(';').map(one).min().unwrap_or(Access::Closed)
}

pub fn granted(value: &str) -> bool {
    classify(value) == Access::Open
}

/// Road class and default access, before explicit tags and one-way rules. The modes are the
/// worldwide defaults, unless the way's country differs.
pub fn highway_access(highway: &str, country: Country) -> Option<(u8, u8)> {
    let (class, modes) = match highway {
        "cycleway" => (0, BIKE),
        "residential" | "living_street" | "unclassified" | "service" | "tertiary" | "tertiary_link" => {
            (1, BIKE | FOOT | PUSH)
        }
        "trunk" | "trunk_link" | "primary" | "primary_link" | "secondary" | "secondary_link" => (2, BIKE | FOOT | PUSH),
        "motorway" | "motorway_link" => (2, 0),
        "track" | "road" => (3, BIKE | FOOT | PUSH),
        "path" => (4, BIKE | FOOT | PUSH),
        "footway" | "pedestrian" => (4, FOOT | PUSH),
        "bridleway" => (4, 0),
        "steps" => (5, FOOT | PUSH),
        _ => return None,
    };
    Some((class, country.defaults(highway).unwrap_or(modes)))
}

pub fn inherited<'a>(get: &impl Fn(&str) -> Option<&'a str>, mode: &str, direction: &str) -> Option<&'a str> {
    let keys: &[&str] = if mode == "bicycle" { &["access", "vehicle", "bicycle"] } else { &["access", "foot"] };
    let mut result = None;
    for key in keys {
        result = get(key).or(result);
        result = get(&format!("{key}:{direction}")).or(result);
    }
    result
}

/// The direction that a one-way rule leaves open for a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oneway {
    Both,
    Forward,
    Backward,
    /// The direction changes over the day; the route reports it as a closure.
    Reversible,
    Unknown,
}

fn oneway_value(value: &str) -> Oneway {
    match value {
        "yes" | "1" | "true" => Oneway::Forward,
        "-1" | "reverse" => Oneway::Backward,
        // Traffic takes turns, as on a one-lane bridge with lights.
        "no" | "0" | "false" | "alternating" => Oneway::Both,
        "reversible" => Oneway::Reversible,
        _ => Oneway::Unknown,
    }
}

/// The road's own one-way rule; a roundabout implies one.
fn road_oneway<'a>(get: &impl Fn(&str) -> Option<&'a str>) -> Oneway {
    let implied = get("junction") == Some("roundabout");
    oneway_value(get("oneway").unwrap_or(if implied { "yes" } else { "no" }))
}

/// The one-way rule for `mode` ("bicycle" or "foot"). `oneway` binds riders only, and not where a
/// cycle lane runs against it.
pub fn oneway<'a>(get: impl Fn(&str) -> Option<&'a str>, mode: &str) -> Oneway {
    if mode == "foot" {
        return get("oneway:foot").map_or(Oneway::Both, oneway_value);
    }
    if let Some(value) = get("oneway:bicycle") {
        return oneway_value(value);
    }
    match road_oneway(&get) {
        // A lane on a one-way road follows the road, so the side of traffic does not matter.
        Oneway::Forward if cycleway(&get, true, false) => Oneway::Both,
        Oneway::Backward if cycleway(&get, false, false) => Oneway::Both,
        oneway => oneway,
    }
}

/// Whether a cycle lane runs in the travel direction. A lane on a two-way road without a direction
/// runs with the traffic on its side of the road.
pub fn cycleway<'a>(get: impl Fn(&str) -> Option<&'a str>, reversed: bool, left_hand: bool) -> bool {
    let road_direction = match road_oneway(&get) {
        Oneway::Forward => Some(false),
        Oneway::Backward => Some(true),
        _ => None,
    };
    ["cycleway", "cycleway:left", "cycleway:right"].iter().any(|key| {
        let both = *key != "cycleway";
        let value = get(key).or_else(|| both.then(|| get("cycleway:both")).flatten()).unwrap_or("");
        if !matches!(
            value,
            "lane"
                | "track"
                | "shared_lane"
                | "share_busway"
                | "shoulder"
                | "opposite"
                | "opposite_lane"
                | "opposite_track"
        ) {
            return false;
        }
        match get(&format!("{key}:oneway")).or_else(|| both.then(|| get("cycleway:both:oneway")).flatten()) {
            Some("no" | "0" | "false") => true,
            Some("yes" | "1" | "true") => !reversed,
            Some("-1" | "reverse") => reversed,
            Some(_) => false,
            None if value.starts_with("opposite") => reversed != road_direction.unwrap_or(false),
            None => road_direction.map_or(
                match *key {
                    "cycleway:left" => reversed != left_hand,
                    "cycleway:right" => reversed == left_hand,
                    _ => true,
                },
                |direction| reversed == direction,
            ),
        }
    })
}

/// Pushing follows pedestrian access, unless explicitly restricted.
/// With `routing`, an uncertain value opens its mode, and the route reports it as a closure.
pub fn access<'a>(get: impl Fn(&str) -> Option<&'a str>, defaults: u8, direction: &str, routing: bool) -> u8 {
    let opens = |value: &str| match classify(value) {
        Access::Open => true,
        Access::Uncertain(_) => routing,
        Access::Closed => false,
    };
    let restricted_road =
        get("motorroad") == Some("yes") || matches!(get("highway"), Some("motorway" | "motorway_link"));
    let mut result = if restricted_road { 0 } else { defaults };
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        if let Some(value) = inherited(&get, mode, direction) {
            if opens(value) {
                result |= bits;
            } else {
                result &= !bits;
            }
        }
        if restricted_road && !get(&format!("{mode}:{direction}")).or(get(mode)).is_some_and(granted) {
            result &= !bits;
        }
    }
    if !restricted_road
        && inherited(&get, "bicycle", direction) == Some("dismount")
        && get(&format!("foot:{direction}")).or(get("foot")).is_none_or(granted)
    {
        result |= FOOT | PUSH;
    }
    if !restricted_road && inherited(&get, "bicycle", direction).is_none() && get("bicycle_road") == Some("yes") {
        result |= BIKE;
    }
    if let Some(value) = get(&format!("bicycle:pushing:{direction}")).or(get("bicycle:pushing")) {
        if opens(value) {
            result |= PUSH;
        } else {
            result &= !PUSH;
        }
    }
    if result & FOOT == 0 {
        result &= !PUSH;
    }
    result
}

/// Modes with a conditional restriction. The router keeps them open; the map shows them.
pub fn conditional_modes<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> u8 {
    conditions(tags).filter(|(_, _, clauses)| !clauses.is_empty()).fold(0, |modes, (_, m, _)| modes | m)
}

/// What may close a road that the router keeps open: an uncertain access value, a reversible
/// one-way, or a conditional restriction. Only a restricting clause counts: `yes @ (May-Oct)`
/// names the open season. A conditional one-way closes one direction only, so it is no closure of
/// the road. Only the tags of the given `directions` of travel count.
pub fn closures<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>, directions: &[&str]) -> Vec<(u8, Closure)> {
    let tags: BTreeMap<&str, &str> = tags.collect();
    let get = |key: &str| tags.get(key).copied();
    let mut found = Vec::new();
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        for &direction in directions {
            if let Some(value) = inherited(&get, mode, direction) {
                if let Access::Uncertain(kind) = classify(value) {
                    found.push((bits, kind, value));
                }
            }
        }
    }
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        if oneway(get, mode) == Oneway::Reversible {
            found.push((bits, Kind::Unclear, "oneway=reversible"));
        }
    }
    for (key, modes, clauses) in conditions(tags.iter().map(|(key, value)| (*key, *value))) {
        let other = ["forward", "backward"].into_iter().find(|d| key.ends_with(d) && !directions.contains(d));
        if modes == 0 || key.starts_with("oneway") || other.is_some() {
            continue;
        }
        for (_, condition) in clauses.into_iter().filter(|(value, _)| !granted(value)) {
            found.push((modes, if seasonal(condition) { Kind::Seasonal } else { Kind::Conditional }, condition));
        }
    }
    let mut closures: Vec<(u8, Closure)> = Vec::new();
    for (modes, kind, condition) in found {
        match closures.iter_mut().find(|(_, known)| known.kind == kind && known.condition == condition) {
            Some((known, _)) => *known |= modes,
            None => closures.push((modes, Closure { kind, condition: condition.to_owned() })),
        }
    }
    closures
}

/// The key, modes and `(value, condition)` clauses of each conditional tag. A hazardous-load
/// clause never concerns a rider or walker, so it is left out.
fn conditions<'a, I: Iterator<Item = (&'a str, &'a str)>>(
    tags: I,
) -> impl Iterator<Item = (&'a str, u8, Vec<(&'a str, &'a str)>)> + use<'a, I> {
    tags.filter_map(|(key, value)| {
        let key = key.strip_suffix(":conditional")?;
        let modes = match key {
            "access" | "access:forward" | "access:backward" => BIKE | FOOT | PUSH,
            "vehicle" | "vehicle:forward" | "vehicle:backward" | "bicycle" | "bicycle:forward" | "bicycle:backward"
            | "oneway" | "oneway:bicycle" => BIKE,
            "bicycle:pushing" | "bicycle:pushing:forward" | "bicycle:pushing:backward" => PUSH,
            "foot" | "foot:forward" | "foot:backward" | "oneway:foot" => FOOT | PUSH,
            _ => 0,
        };
        let clauses = value
            .split(';')
            .map(|clause| {
                let (value, condition) = clause.split_once('@').unwrap_or((clause, ""));
                (value.trim(), condition.trim().trim_matches(['(', ')']).trim())
            })
            .filter(|(_, condition)| !hazmat(condition))
            .collect();
        Some((key, modes, clauses))
    })
}

/// A hazardous-load condition never concerns a rider or walker.
fn hazmat(condition: &str) -> bool {
    matches!(condition, "hazmat" | "hazmat:water")
}

/// A closure such as `Nov-May` moves with the snow each year; the snow layer tells the rider
/// when the road is usually clear. A dated closure with a year is not seasonal.
fn seasonal(condition: &str) -> bool {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    const SEASONS: [&str; 4] = ["winter", "spring", "summer", "autumn"];
    let condition = condition.to_ascii_lowercase();
    let mut named = false;
    let dates = condition.split([' ', '-', ',']).filter(|token| !token.is_empty()).all(|token| {
        let day = token.trim_end_matches(char::is_alphabetic);
        if !day.is_empty() {
            let suffix = &token[day.len()..];
            return matches!(suffix, "" | "st" | "nd" | "rd" | "th")
                && day.parse::<u8>().is_ok_and(|d| (1..=31).contains(&d));
        }
        let name = SEASONS.contains(&token) || MONTHS.iter().any(|m| token.starts_with(m));
        named |= name;
        name
    });
    dates && named
}

/// The tags of one object, packed in key order: for each pair, the key length and the value
/// length as little-endian `u16`, then the key bytes and the value bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags<'a>(Cow<'a, [u8]>);

fn length(text: &str) -> [u8; 2] {
    u16::try_from(text.len()).expect("OSM limits a text to 255 characters").to_le_bytes()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("only text enters the packed bytes")
}

impl Tags<'_> {
    /// A repeated key keeps its last value.
    pub fn from_pairs<'p>(pairs: impl IntoIterator<Item = (&'p str, &'p str)>) -> Tags<'static> {
        let mut pairs: Vec<_> = pairs.into_iter().collect();
        pairs.reverse();
        pairs.sort_by_key(|(key, _)| *key);
        pairs.dedup_by_key(|(key, _)| *key);
        let mut bytes = Vec::new();
        for (key, value) in pairs {
            bytes.extend(length(key));
            bytes.extend(length(value));
            bytes.extend(key.as_bytes());
            bytes.extend(value.as_bytes());
        }
        Tags(Cow::Owned(bytes))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.packed().find(|(k, _)| *k == key.as_bytes()).map(|(_, value)| text(value))
    }

    pub fn contains(&self, key: &str, value: &str) -> bool {
        self.get(key) == Some(value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> + Clone {
        self.packed().map(|(key, value)| (text(key), text(value)))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn packed(&self) -> impl Iterator<Item = (&[u8], &[u8])> + Clone {
        let mut rest = &self.0[..];
        std::iter::from_fn(move || {
            let (&[k0, k1, v0, v1], tail) = rest.split_first_chunk()?;
            let (key, tail) = tail.split_at(u16::from_le_bytes([k0, k1]).into());
            let (value, tail) = tail.split_at(u16::from_le_bytes([v0, v1]).into());
            rest = tail;
            Some((key, value))
        })
    }
}

/// What one node reference of a way gets from its node: the point and what the node's tags give.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeRef {
    /// Microdegrees, as in `Point`.
    pub lat: i32,
    pub lon: i32,
    /// The `Sets` ID of `osm::node_closures`, or `u32::MAX`.
    pub closures: u32,
    /// The modes that pass the node (`osm::crossing`); all modes for an untagged node.
    pub access: u8,
    pub flags: u8,
    pub padding: [u8; 2],
}

const _: () = assert!(size_of::<NodeRef>() == 4 + 4 + 4 + 1 + 1 + 2);

impl NodeRef {
    /// The node is in the input and inside the bounds.
    pub const PRESENT: u8 = 1;
    /// More than one way reference uses the node.
    pub const SHARED: u8 = 2;
    /// A signal, a crossing limit or a closure: the node ends its roads.
    pub const MARKED: u8 = 4;
    pub const SIGNALS: u8 = 8;

    /// A reference to a node that is not in the input or not inside the bounds.
    pub fn missing(shared: bool) -> Self {
        let flags = if shared { Self::SHARED } else { 0 };
        NodeRef { lat: 0, lon: 0, closures: u32::MAX, access: BIKE | FOOT | PUSH, flags, padding: [0; 2] }
    }

    pub fn present(&self) -> bool {
        self.flags & Self::PRESENT != 0
    }

    pub fn point(&self) -> Point {
        Point { lat: self.lat, lon: self.lon, elevation: NO_ELEVATION }
    }
}

/// What the tags of a way give with no profile. Each field is a byte or a `u32`, so a stored
/// column holds it; the methods decode them.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WayFacts {
    /// The `Sets` IDs of `closures` forward and backward, or `u32::MAX`.
    pub closures: [u32; 2],
    /// 1 when `osm::attributes` gave attributes: the way can have roads.
    pub routable: u8,
    pub class: u8,
    /// A `Surface` (`surface_byte`).
    pub surface: u8,
    /// The modes forward and backward, after the exclusions of the via-way restrictions.
    pub access: [u8; 2],
    pub difficulty: u8,
    /// `hiking_byte` of the hiking difficulty.
    pub hiking_difficulty: u8,
    /// 1 for a bridge or a tunnel.
    pub structure: u8,
    /// The `Country` of the first present node: its row of defaults, and 1 for left-hand traffic.
    pub defaults: u8,
    pub left_hand: u8,
    /// The doubtful modes of those sets (`cost::doubts`).
    pub doubts: [u8; 2],
    /// The highest cycle route rank forward and backward, 0 for none. A way in a cycle route has a
    /// rank in at least one direction.
    pub ranks: [u8; 2],
    pub padding: [u8; 2],
}

const _: () = assert!(size_of::<WayFacts>() == 8 + 1 + 1 + 1 + 2 + 1 + 1 + 1 + 1 + 1 + 2 + 2 + 2);

impl Default for WayFacts {
    fn default() -> Self {
        let (defaults, left_hand) = Country::default().parts();
        WayFacts {
            closures: [u32::MAX; 2],
            routable: 0,
            class: 0,
            surface: surface_byte(Surface::Unknown),
            access: [0; 2],
            difficulty: 255,
            hiking_difficulty: hiking_byte(None),
            structure: 0,
            defaults,
            left_hand,
            doubts: [0; 2],
            ranks: [0; 2],
            padding: [0; 2],
        }
    }
}

impl WayFacts {
    pub fn routable(&self) -> bool {
        self.routable != 0
    }

    pub fn surface(&self) -> Surface {
        surface(self.surface)
    }

    pub fn hiking_difficulty(&self) -> Option<u8> {
        hiking(self.hiking_difficulty)
    }

    pub fn structure(&self) -> bool {
        self.structure != 0
    }

    pub fn country(&self) -> Country {
        Country::from_parts(self.defaults, self.left_hand)
    }
}

pub fn surface_byte(surface: Surface) -> u8 {
    surface as u8
}

pub fn surface(byte: u8) -> Surface {
    match byte {
        1 => Surface::Paved,
        2 => Surface::Compacted,
        3 => Surface::Gravel,
        4 => Surface::Dirt,
        5 => Surface::Rough,
        _ => Surface::Unknown,
    }
}

/// The byte of a hiking difficulty: `0xFE` for none. An unknown classification is 255.
pub fn hiking_byte(difficulty: Option<u8>) -> u8 {
    difficulty.unwrap_or(0xFE)
}

pub fn hiking(byte: u8) -> Option<u8> {
    (byte != 0xFE).then_some(byte)
}

/// A turn rule of a relation: from way `from` over node `via` to way `to`. `only` forbids the
/// other turns, `u_turn` takes only the reverse road of `to`, and `walking` binds the foot modes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub from: i64,
    pub to: i64,
    pub via: i64,
    pub only: u8,
    pub u_turn: u8,
    pub walking: u8,
    pub padding: [u8; 5],
}

const _: () = assert!(size_of::<Rule>() == 8 + 8 + 8 + 1 + 1 + 1 + 5);

/// The distinct closure sets of the bake: of the ways, of the nodes and of the roads. An empty set
/// has the ID `u32::MAX`.
#[derive(Clone, Debug, Default)]
pub struct Sets {
    sets: Vec<Set>,
    ids: HashMap<Set, u32>,
}

impl Sets {
    /// Keeps the first copy of a closure that the set repeats.
    pub fn insert(&mut self, set: Set) -> u32 {
        let mut unique: Set = Vec::with_capacity(set.len());
        for closure in set {
            if !unique.contains(&closure) {
                unique.push(closure);
            }
        }
        if unique.is_empty() {
            return u32::MAX;
        }
        if let Some(&id) = self.ids.get(&unique) {
            return id;
        }
        let id =
            u32::try_from(self.sets.len()).ok().filter(|&id| id != u32::MAX).expect("fewer than 2^32 - 1 closure sets");
        self.ids.insert(unique.clone(), id);
        self.sets.push(unique);
        id
    }

    pub fn get(&self, id: u32) -> &[(u8, Closure)] {
        self.sets.get(id as usize).map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.sets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// The sets in ID order, as postcard.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(&self.sets).expect("closure sets encode")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let sets: Vec<Set> = postcard::from_bytes(bytes).map_err(|e| format!("closure sets: {e}"))?;
        let ids = sets.iter().enumerate().map(|(id, set)| (set.clone(), id as u32)).collect();
        Ok(Sets { sets, ids })
    }
}

#[derive(Clone, Debug)]
pub struct Way<'a> {
    pub id: i64,
    pub nodes: &'a [i64],
    /// One for each node, after the join of the nodes; empty before.
    pub points: &'a [NodeRef],
    pub tags: Tags<'a>,
    pub facts: WayFacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Node(i64),
    Way(i64),
    Relation(i64),
}

impl Id {
    /// The stored kind byte of a relation member, and its ID. `decode` reverses it.
    fn encode(self) -> (u8, i64) {
        match self {
            Id::Node(id) => (0, id),
            Id::Way(id) => (1, id),
            Id::Relation(id) => (2, id),
        }
    }

    fn decode(kind: u8, id: i64) -> Self {
        match kind {
            0 => Id::Node(id),
            1 => Id::Way(id),
            2 => Id::Relation(id),
            _ => unreachable!("`encode` writes only the kinds 0, 1 and 2"),
        }
    }

    pub fn way(self) -> Option<i64> {
        if let Id::Way(id) = self {
            Some(id)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct Relation<'a> {
    pub id: i64,
    pub tags: Tags<'a>,
    members: &'a [i64],
    /// For each member, its kind as one byte, then its role as a `u16` length and the bytes.
    roles: &'a [u8],
}

impl<'a> Relation<'a> {
    pub fn members(&self) -> impl Iterator<Item = (Id, &'a str)> + 'a {
        let mut roles = self.roles;
        self.members.iter().map(move |&id| {
            let (&[kind, l0, l1], tail) = roles.split_first_chunk().expect("each member has a role");
            let (role, tail) = tail.split_at(u16::from_le_bytes([l0, l1]).into());
            roles = tail;
            (Id::decode(kind, id), text(role))
        })
    }
}

/// The columns of one kind of object, apart from the IDs.
pub trait Columns: Default {
    type Row<'a>
    where
        Self: 'a;
    fn row(&self, id: i64, index: usize) -> Self::Row<'_>;
    /// Appends row `index` of `from`.
    fn copy(&mut self, from: &Self, index: usize);
}

/// Objects sorted by OSM ID, each ID once.
#[derive(Clone, Debug, Default)]
pub struct Table<C> {
    ids: Column<i64>,
    columns: C,
}

impl<C: Columns> Table<C> {
    pub fn len(&self) -> usize {
        self.ids.values().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: i64) -> Option<C::Row<'_>> {
        self.index(id).map(|index| self.row(index))
    }

    /// The row of `id`.
    pub fn index(&self, id: i64) -> Option<usize> {
        self.ids.values().binary_search(&id).ok()
    }

    /// In ID order.
    pub fn iter(&self) -> impl Iterator<Item = C::Row<'_>> + '_ {
        (0..self.len()).map(|index| self.row(index))
    }

    /// A copy with only the rows that `keep` accepts.
    #[must_use]
    pub fn filter(&self, mut keep: impl FnMut(&C::Row<'_>) -> bool) -> Self {
        self.select((0..self.len()).filter(|&index| keep(&self.row(index))))
    }

    pub(crate) fn row(&self, index: usize) -> C::Row<'_> {
        self.columns.row(self.ids.values()[index], index)
    }

    fn select(&self, indices: impl Iterator<Item = usize>) -> Self {
        let mut table = Self::default();
        for index in indices {
            table.ids.push(self.ids.values()[index]);
            table.columns.copy(&self.columns, index);
        }
        table
    }
}

/// Objects in any order, until `finish` sorts them.
#[derive(Default)]
pub struct Builder<C>(Table<C>);

impl<C: Columns> Builder<C> {
    /// Sorts the objects by ID and keeps the first copy of each ID. Also returns the number of
    /// copies that it dropped.
    pub fn finish(self) -> (Table<C>, usize) {
        let ids = self.0.ids.values();
        let order = ids.iter().enumerate().map(|(index, &id)| (id, index as u64)).collect();
        let mut previous = None;
        let first = sort::sorted(order).filter(|&(id, _)| previous.replace(id) != Some(id));
        let table = self.0.select(first.map(|(_, index)| index as usize));
        let dropped = ids.len() - table.len();
        (table, dropped)
    }
}

/// `points` and `facts` are empty until the import joins the nodes and scans the ways.
#[derive(Clone, Debug, Default)]
pub struct Ways {
    nodes: Lists<i64>,
    /// One for each node reference, at the position of its node ID in `nodes`.
    points: Column<NodeRef>,
    tags: Lists<u8>,
    facts: Column<WayFacts>,
}

impl Columns for Ways {
    type Row<'a> = Way<'a>;

    fn row(&self, id: i64, index: usize) -> Way<'_> {
        let points = if self.points.is_empty() { &[] } else { &self.points.values()[self.nodes.range(index)] };
        Way {
            id,
            nodes: self.nodes.get(index),
            points,
            tags: Tags(Cow::Borrowed(self.tags.get(index))),
            facts: self.facts.values().get(index).copied().unwrap_or_default(),
        }
    }

    fn copy(&mut self, from: &Self, index: usize) {
        self.nodes.push(from.nodes.get(index));
        self.tags.push(from.tags.get(index));
        if !from.points.is_empty() {
            self.points.extend(&from.points.values()[from.nodes.range(index)]);
        }
        if !from.facts.is_empty() {
            self.facts.push(from.facts.values()[index]);
        }
    }
}

impl Table<Ways> {
    /// The node IDs of all ways, way after way: a reference position is an index into it.
    pub fn references(&self) -> &[i64] {
        self.columns.nodes.values()
    }

    /// `points` has one value for each reference position.
    pub fn set_points(&mut self, points: Column<NodeRef>) {
        assert_eq!(points.len(), self.references().len(), "one point for each node reference");
        self.columns.points = points;
    }

    /// `facts` has one value for each way, in ID order.
    pub fn set_facts(&mut self, facts: Column<WayFacts>) {
        assert_eq!(facts.len(), self.len(), "facts for each way");
        self.columns.facts = facts;
    }
}

impl Builder<Ways> {
    pub fn push(&mut self, id: i64, nodes: &[i64], tags: &Tags) {
        self.0.ids.push(id);
        self.0.columns.nodes.push(nodes);
        self.0.columns.tags.push(&tags.0);
    }
}

#[derive(Clone, Debug, Default)]
pub struct Relations {
    members: Lists<i64>,
    roles: Lists<u8>,
    tags: Lists<u8>,
}

impl Columns for Relations {
    type Row<'a> = Relation<'a>;

    fn row(&self, id: i64, index: usize) -> Relation<'_> {
        Relation {
            id,
            tags: Tags(Cow::Borrowed(self.tags.get(index))),
            members: self.members.get(index),
            roles: self.roles.get(index),
        }
    }

    fn copy(&mut self, from: &Self, index: usize) {
        self.members.push(from.members.get(index));
        self.roles.push(from.roles.get(index));
        self.tags.push(from.tags.get(index));
    }
}

impl Builder<Relations> {
    pub fn push<'r>(&mut self, id: i64, tags: &Tags, members: impl IntoIterator<Item = (Id, &'r str)>) {
        let (mut ids, mut roles) = (Vec::new(), Vec::new());
        for (member, role) in members {
            let (kind, id) = member.encode();
            ids.push(id);
            roles.push(kind);
            roles.extend(length(role));
            roles.extend(role.as_bytes());
        }
        self.0.ids.push(id);
        self.0.columns.members.push(&ids);
        self.0.columns.roles.push(&roles);
        self.0.columns.tags.push(&tags.0);
    }
}

/// The source objects of the import.
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub ways: Table<Ways>,
    pub relations: Table<Relations>,
    /// The node members of the relations that a way references and that are present, sorted.
    pub members: Column<i64>,
}

impl Data {
    /// Saves each column as a file whose name starts with `ways.`, `relations.` or `members`.
    pub(crate) fn save(self, layout: &mut Layout) -> Result<(), String> {
        let Data { ways, relations, members } = self;
        let Ways { nodes, points, tags, facts } = ways.columns;
        layout.save("ways.ids", ways.ids)?;
        layout.save_lists("ways.nodes", nodes)?;
        layout.save("ways.points", points)?;
        layout.save_lists("ways.tags", tags)?;
        layout.save("ways.facts", facts)?;
        let Relations { members: ids, roles, tags } = relations.columns;
        layout.save("relations.ids", relations.ids)?;
        layout.save_lists("relations.members", ids)?;
        layout.save_lists("relations.roles", roles)?;
        layout.save_lists("relations.tags", tags)?;
        layout.save("members", members)
    }

    pub(crate) fn open(layout: &Layout) -> Result<Self, String> {
        let ways = Ways {
            nodes: layout.open_lists("ways.nodes")?,
            points: layout.open("ways.points")?,
            tags: layout.open_lists("ways.tags")?,
            facts: layout.open("ways.facts")?,
        };
        let relations = Relations {
            members: layout.open_lists("relations.members")?,
            roles: layout.open_lists("relations.roles")?,
            tags: layout.open_lists("relations.tags")?,
        };
        Ok(Data {
            ways: Table { ids: layout.open("ways.ids")?, columns: ways },
            relations: Table { ids: layout.open("relations.ids")?, columns: relations },
            members: layout.open("members")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oneway_binds_riders_unless_a_lane_or_tag_frees_them() {
        let rule =
            |tags: &[(&str, &str)], mode| oneway(|key| tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v), mode);
        assert_eq!(rule(&[("oneway", "yes")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("oneway", "yes")], "foot"), Oneway::Both);
        assert_eq!(rule(&[("junction", "roundabout")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("junction", "roundabout"), ("oneway", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("junction", "circular")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("junction", "circular"), ("oneway", "yes")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("junction", "circular"), ("oneway", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "alternating")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "-1"), ("oneway:bicycle", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "yes"), ("cycleway:left", "opposite_lane")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "yes"), ("cycleway:left", "lane")], "bicycle"), Oneway::Forward);
        let reversible = closures([("oneway", "reversible")].into_iter(), &["forward"]);
        assert_eq!(reversible, vec![(BIKE, Closure { kind: Kind::Unclear, condition: "oneway=reversible".into() })]);
    }

    #[test]
    fn a_lane_without_a_direction_runs_with_the_traffic_on_its_side() {
        let get = |key: &str| (key == "cycleway:left").then_some("lane");
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, false)), [false, true]);
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, true)), [true, false]);
    }

    #[test]
    fn only_no_closes_and_every_other_restriction_becomes_a_closure() {
        use Access::{Closed, Open, Uncertain};
        for (value, expected) in [
            ("yes;designated", Open),
            ("mtb", Open),
            ("optional_sidepath", Open),
            ("no", Closed),
            ("dismount", Closed),
            ("permit", Uncertain(Kind::Permit)),
            ("private", Uncertain(Kind::Private)),
            ("agricultural;forestry", Uncertain(Kind::Farm)),
            ("use_sidepath", Uncertain(Kind::Sidepath)),
            ("discouraged", Uncertain(Kind::Discouraged)),
            ("customers", Uncertain(Kind::Limited)),
            ("motor_vehicle;emergency", Uncertain(Kind::Limited)),
            ("military", Uncertain(Kind::Limited)),
            ("no;private", Uncertain(Kind::Private)),
            ("service", Uncertain(Kind::Unclear)),
        ] {
            assert_eq!(classify(value), expected, "{value}");
        }
        let permit = |key: &str| (key == "access").then_some("permit");
        assert_eq!(access(permit, BIKE | FOOT | PUSH, "forward", true), BIKE | FOOT | PUSH);
        assert_eq!(access(permit, BIKE | FOOT | PUSH, "forward", false), 0);

        let closures = |tags: &[(&'static str, &'static str)]| closures(tags.iter().copied(), &["forward", "backward"]);
        let closure = |kind, condition: &str| Closure { kind, condition: condition.into() };
        let all = BIKE | FOOT | PUSH;
        assert_eq!(
            closures(&[
                ("access", "permit"),
                ("vehicle", "permit"),
                ("access:conditional", "no @ Oct 14th - May 31st")
            ]),
            vec![(all, closure(Kind::Permit, "permit")), (all, closure(Kind::Seasonal, "Oct 14th - May 31st"))]
        );
        assert_eq!(
            closures(&[("access", "destination"), ("bicycle", "yes"), ("bicycle:conditional", "no @ (wet)")]),
            vec![(FOOT | PUSH, closure(Kind::Limited, "destination")), (BIKE, closure(Kind::Conditional, "wet"))]
        );
        assert_eq!(
            closures(&[("access", "private"), ("bicycle", "use_sidepath")]),
            vec![(FOOT | PUSH, closure(Kind::Private, "private")), (BIKE, closure(Kind::Sidepath, "use_sidepath"))]
        );
        let one_way = [("bicycle:backward", "use_sidepath"), ("bicycle:backward:conditional", "no @ (wet)")];
        assert_eq!(super::closures(one_way.into_iter(), &["forward"]), vec![]);
        assert_eq!(super::closures(one_way.into_iter(), &["backward"]).len(), 2);
        assert_eq!(
            closures(&[
                ("access:conditional", "no @ (Nov-May)"),
                ("foot:conditional", "no @ (2026 Mar 1-2026 Jul 31)")
            ]),
            vec![
                (all, closure(Kind::Seasonal, "Nov-May")),
                (FOOT | PUSH, closure(Kind::Conditional, "2026 Mar 1-2026 Jul 31"))
            ]
        );
        // An open season, a one-way, a motor-vehicle rule and a freight rule close nothing.
        for tag in [
            ("access:conditional", "yes @ (May-Oct)"),
            ("oneway:conditional", "yes @ (Mo-Fr 07:00-09:00)"),
            ("motor_vehicle:conditional", "no @ (wet)"),
            ("access:conditional", "agricultural @ hazmat:water"),
        ] {
            assert_eq!(closures(&[tag]), vec![], "{tag:?}");
        }
    }

    #[test]
    fn tags_keep_the_key_order_and_find_every_key() {
        let tags = Tags::from_pairs([("name", "old"), ("highway", "path"), ("a\0b", ""), ("name", "A\0B")]);
        assert_eq!(tags.iter().collect::<Vec<_>>(), [("a\0b", ""), ("highway", "path"), ("name", "A\0B")]);
        assert!(tags.iter().all(|(key, value)| tags.get(key) == Some(value)));
        assert_eq!((tags.get("a"), tags.get("nam")), (None, None));
    }

    #[test]
    fn a_builder_sorts_by_id_and_keeps_the_first_copy() {
        let tags = |value: &str| if value.is_empty() { Tags::default() } else { Tags::from_pairs([("k", value)]) };
        let mut ways = Builder::<Ways>::default();
        for (id, value) in [(3, "first"), (1, ""), (3, "second"), (2, "two")] {
            ways.push(id, &[id], &tags(value));
        }
        let (ways, dropped) = ways.finish();
        let rows: Vec<_> = ways.iter().map(|way| (way.id, way.tags)).collect();
        assert_eq!((rows, dropped), (vec![(1, tags("")), (2, tags("two")), (3, tags("first"))], 1));
    }
}
