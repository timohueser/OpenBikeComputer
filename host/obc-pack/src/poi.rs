//! OSM point-of-interest extraction: classify nodes and closed ways against the fixed
//! category/subtype table, normalize names for the device font, and collapse OSM double-mapping.
//!
//! The table below is canonical and append-only: ids are stable, mirrored in firmware, and pinned
//! normatively in `OBCM_Spec.md`. Subtype `0` is reserved, and `0xFF` is the end-of-chunk sentinel.
//! First match in table order wins, the same convention as the config's style map.
//!
//! This stage is deliberately config-free: the tag mapping is hardcoded, so packing the same extract
//! always yields the same POIs.

use std::collections::HashMap;

use obc_formats::obcm::{
    poi_directory_category_of, poi_label_of, settlement_class_of, POI_NAME_LEN, SETTLEMENT_POPULATION_UNKNOWN,
    SETTLEMENT_SUBTYPE_CITY, SETTLEMENT_SUBTYPE_HAMLET, SETTLEMENT_SUBTYPE_TOWN, SETTLEMENT_SUBTYPE_VILLAGE,
    SUMMIT_SUBTYPE_ID,
};

/// The largest population the payload can hold: every value below the unknown one is real.
const SETTLEMENT_POPULATION_MAX: u16 = SETTLEMENT_POPULATION_UNKNOWN - 1;

use crate::hours::Schedule;

/// One row of the canonical table: the OSM `key=value` classification and the subtype id it maps to.
/// The subtype's category and fallback label live once in `obc-formats`, and this row derives them
/// via [`PoiKind::category`] and [`PoiKind::label`], so that mapping is never maintained twice. Only
/// the OSM tag classification, which the device never needs, stays packer-side.
pub struct PoiKind {
    pub subtype: u8,
    pub key: &'static str,
    pub value: &'static str,
}

impl PoiKind {
    /// The category id this subtype belongs to, derived from `obc-formats`' canonical table. Every
    /// `POI_TABLE` subtype is valid there, so the unwrap never trips.
    /// table. Every `POI_TABLE` subtype is valid there, so the unwrap never trips (the pinning test
    pub fn category(&self) -> u8 {
        poi_directory_category_of(self.subtype).expect("POI_TABLE subtype has a directory category")
    }

    /// The device fallback label for this subtype, shown when OSM has no usable name.
    pub fn label(&self) -> &'static str {
        poi_label_of(self.subtype).expect("POI_TABLE subtype is in obc-formats' canonical table")
    }
}

const fn kind(subtype: u8, key: &'static str, value: &'static str) -> PoiKind {
    PoiKind { subtype, key, value }
}

/// The canonical OSM-tag to subtype classification. Subtype ids are normative and append-only:
/// never renumber. The subtype to category and label half lives in `obc-formats`. First match in
/// table order wins (see [`classify`]).
pub const POI_TABLE: [PoiKind; 24] = [
    kind(1, "amenity", "drinking_water"),
    kind(2, "natural", "spring"),
    kind(3, "man_made", "water_tap"),
    kind(4, "amenity", "water_point"),
    kind(5, "tourism", "camp_site"),
    kind(6, "tourism", "caravan_site"),
    kind(7, "tourism", "hotel"),
    kind(8, "tourism", "hostel"),
    kind(9, "tourism", "guest_house"),
    kind(10, "tourism", "motel"),
    kind(11, "tourism", "wilderness_hut"),
    kind(12, "tourism", "alpine_hut"),
    kind(13, "shop", "supermarket"),
    kind(14, "shop", "convenience"),
    kind(15, "shop", "bakery"),
    kind(16, "amenity", "marketplace"),
    kind(17, "amenity", "pharmacy"),
    kind(18, "shop", "bicycle"),
    kind(SUMMIT_SUBTYPE_ID, "natural", "peak"),
    kind(20, "railway", "station"),
    // Settlements come last: a node can carry both `place=village` and a service tag, and the
    // service tag is the one the rider looks for.
    kind(SETTLEMENT_SUBTYPE_CITY, "place", "city"),
    kind(SETTLEMENT_SUBTYPE_TOWN, "place", "town"),
    kind(SETTLEMENT_SUBTYPE_VILLAGE, "place", "village"),
    kind(SETTLEMENT_SUBTYPE_HAMLET, "place", "hamlet"),
];

/// Category display names for the pack log, indexed by category id (0 unused).
pub const CATEGORY_NAMES: [&str; 10] = [
    "",
    "water",
    "campsite",
    "accommodation",
    "resupply",
    "pharmacy",
    "bike shop",
    "summit",
    "train station",
    "settlement",
];

/// A classified POI candidate. Coordinates are µdeg (`round(deg * 1e6)`), the
/// same grid the serializer's chunk coords live on.
#[derive(Debug, Clone, PartialEq)]
pub struct Poi {
    pub metadata: obc_formats::obcm::PoiMetadata,
    /// Explicit topology and article links retained only at build time.
    pub access_nodes: Vec<i64>,
    pub wikidata: Option<String>,
    pub wikipedia: Option<String>,
    pub subtype: u8,
    pub lon_udeg: i32,
    pub lat_udeg: i32,
    /// At most 24 bytes: ASCII-folded service names or UTF-8 summit names.
    pub name: Option<String>,
    /// Nodes mark entrances; way-centroids are derived. Drives dedup priority.
    pub from_node: bool,
    /// Parsed weekly schedule from the OSM `opening_hours` tag, or `None` when the POI has none
    /// that parses. The serializer pools these and stores a `hours_ref` on the record.
    pub hours: Option<Schedule>,
    /// Summit height in metres, from OSM `ele` or the shared DEM when the tag is absent.
    pub elevation_m: Option<i16>,
    /// Settlement population in whole people, when the source gives a usable value.
    pub population: Option<u32>,
}

/// Explicit source links for offline landmark preparation, including entities with no service category.
#[derive(Debug, Clone)]
pub struct LandmarkLink {
    pub metadata: obc_formats::obcm::PoiMetadata,
    pub position: Option<(i32, i32)>,
    pub wikidata: Option<String>,
    pub wikipedia: Option<String>,
    pub hours: Option<Schedule>,
}

impl From<&Poi> for LandmarkLink {
    fn from(poi: &Poi) -> Self {
        Self {
            metadata: poi.metadata,
            position: Some((poi.lon_udeg, poi.lat_udeg)),
            wikidata: poi.wikidata.clone(),
            wikipedia: poi.wikipedia.clone(),
            hours: poi.hours.clone(),
        }
    }
}

/// Build-only subtype zero carries an explicit Wiki link through source topology resolution.
pub(crate) fn classify_linked<'a>(
    tags: impl IntoIterator<Item = (&'a str, &'a str)> + Clone,
) -> Option<Classification<'a>> {
    classify(tags.clone()).or_else(|| {
        let tags: Vec<_> = tags.into_iter().collect();
        tags.iter().any(|(key, _)| matches!(*key, "wikidata" | "wikipedia")).then(|| Classification {
            subtype: 0,
            name: tags.iter().find(|(key, _)| *key == "name").and_then(|(_, value)| normalize_name(value)),
            raw_hours: tags.iter().find(|(key, _)| *key == "opening_hours").map(|(_, value)| *value),
            elevation_m: None,
            population: None,
        })
    })
}

/// Look up a subtype's table row (subtype ids are 1-based and dense).
pub fn table_row(subtype: u8) -> &'static PoiKind {
    &POI_TABLE[subtype as usize - 1]
}

/// Classified OSM point metadata before schedule parsing and DEM height lookup.
#[derive(Debug, PartialEq)]
pub struct Classification<'a> {
    pub subtype: u8,
    pub name: Option<String>,
    pub raw_hours: Option<&'a str>,
    pub elevation_m: Option<i16>,
    pub population: Option<u32>,
}

/// Classify a tag set against [`POI_TABLE`] — first match in table order wins — and pull the
/// normalized `name` plus the raw `opening_hours` value alongside. One pass over the tags, with no
/// allocation on the overwhelmingly common no-match path. The `opening_hours` string comes back
/// unparsed, as a borrowed slice, so the fast path stays alloc-free.
pub fn classify<'a, I>(tags: I) -> Option<Classification<'a>>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut best: Option<usize> = None;
    let mut raw_name: Option<&str> = None;
    let mut short_name: Option<&str> = None;
    let mut name_en: Option<&str> = None;
    let mut int_name: Option<&str> = None;
    let mut raw_hours: Option<&str> = None;
    let mut elevation_m = None;
    let mut population = None;
    for (k, v) in tags {
        if k == "name" {
            raw_name = Some(v);
            continue;
        }
        if k == "short_name" {
            short_name = Some(v);
            continue;
        }
        if k == "name:en" {
            name_en = Some(v);
            continue;
        }
        if k == "int_name" {
            int_name = Some(v);
            continue;
        }
        if k == "population" {
            // Free text in OSM: keep it only when the whole value is a number once the spaces are
            // removed. A separator or a qualifier, such as `12,000` or `~500`, gives unknown.
            population = v.split_whitespace().collect::<String>().parse::<u32>().ok();
            continue;
        }
        if k == "opening_hours" {
            raw_hours = Some(v);
            continue;
        }
        if k == "ele" {
            let value = v.trim().strip_suffix('m').unwrap_or(v).trim().parse::<f64>().ok();
            elevation_m =
                value.filter(|v| v.is_finite() && (-32767.0..=32767.0).contains(&v.round())).map(|v| v.round() as i16);
            continue;
        }
        for (i, kind) in POI_TABLE.iter().enumerate() {
            if best.is_some_and(|b| b <= i) {
                break;
            }
            if kind.key == k && kind.value == v {
                best = Some(i);
                break;
            }
        }
    }
    let subtype = POI_TABLE[best?].subtype;
    if subtype == SUMMIT_SUBTYPE_ID {
        let name = utf8_record_name(raw_name?)?;
        return Some(Classification { subtype, name: Some(name), raw_hours: None, elevation_m, population: None });
    }
    if settlement_class_of(subtype).is_some() {
        let name = utf8_record_name(&pick_settlement_name(raw_name?, short_name, name_en, int_name)?)?;
        return Some(Classification { subtype, name: Some(name), raw_hours: None, elevation_m: None, population });
    }
    Some(Classification {
        subtype,
        name: raw_name.and_then(normalize_name),
        raw_hours,
        elevation_m: None,
        population: None,
    })
}

/// The record payload of a settlement: the population in hundreds of people, saturating, or
/// [`SETTLEMENT_POPULATION_UNKNOWN`] when the source gave no usable value. The clamp happens before
/// the cast, so a nonsense tag lands on the maximum instead of wrapping.
pub fn settlement_payload(population: Option<u32>) -> u16 {
    population.map_or(SETTLEMENT_POPULATION_UNKNOWN, |n| (n / 100).min(SETTLEMENT_POPULATION_MAX as u32) as u16)
}

/// Cut a UTF-8 name to the record's fixed `Name` field on a character boundary. Summits and
/// settlements keep their own spelling, so this is the whole treatment they get. `None` when the
/// name is empty or holds a control character.
fn utf8_record_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let mut end = name.len().min(POI_NAME_LEN);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name[..end];
    (!name.is_empty() && !name.chars().any(char::is_control)).then(|| name.into())
}

/// Choose the settlement name the device can draw: the local name, else its ASCII fold, else
/// `name:en`, else `int_name`. `None` drops the settlement, because a row of question marks is
/// worse than no label. The fold turns Cyrillic, Greek and CJK into word breaks, so it gives an
/// empty result for them and the language fall-backs run.
///
/// The local name is `short_name` when the source gives a shorter one — the map shows 12
/// characters, so "Freiburg" is the whole label where "Freiburg im Breisgau" is a cut.
fn pick_settlement_name(
    name: &str,
    short_name: Option<&str>,
    name_en: Option<&str>,
    int_name: Option<&str>,
) -> Option<String> {
    let local = short_name
        .map(str::trim)
        .filter(|short| !short.is_empty() && short.chars().count() < name.trim().chars().count())
        .unwrap_or(name);
    // `glyph_supported` reads the real font strip, so the repertoire cannot drift from it.
    let drawable = |name: &&str| name.chars().all(obc_render::glyph_supported);
    if drawable(&local) {
        return Some(local.into());
    }
    normalize_name(local).or_else(|| [name_en, int_name].into_iter().flatten().find(drawable).map(Into::into))
}

/// Fill missing summit heights from the shared geographic terrain lattice.
pub fn fill_summit_elevations(pois: &mut [Poi], terrain: &mut dyn obc_elevation::ElevationSource) {
    for poi in pois {
        if poi.subtype == SUMMIT_SUBTYPE_ID && poi.elevation_m.is_none() {
            poi.elevation_m = terrain.sample(poi.lat_udeg, poi.lon_udeg);
        }
    }
}

/// Convert exact-osmium degrees to the µdeg grid.
pub fn to_udeg(deg: f64) -> i32 {
    (deg * 1e6).round() as i32
}

/// Ring centroid of a closed way (standard shoelace-weighted formula) in
/// degrees. `coords` carries the duplicated closing vertex. Degenerate rings
/// (zero/near-zero area, or fewer than 3 distinct vertices) fall back to the
/// vertex mean over the distinct vertices.
pub fn ring_centroid(coords: &[(f64, f64)]) -> (f64, f64) {
    // Distinct vertices: drop the duplicated closing point.
    let pts = if coords.len() >= 2 && coords.first() == coords.last() { &coords[..coords.len() - 1] } else { coords };
    if pts.len() >= 3 {
        // Shoelace in coordinates local to the first vertex — raw lon/lat
        // products (~400) vs building-sized areas (~1e-7 deg²) would eat the
        // centroid's precision through cancellation (µdeg-level error).
        let (rx, ry) = pts[0];
        let (mut a2, mut cx, mut cy) = (0.0, 0.0, 0.0);
        for i in 0..pts.len() {
            let (x0, y0) = (pts[i].0 - rx, pts[i].1 - ry);
            let j = (i + 1) % pts.len();
            let (x1, y1) = (pts[j].0 - rx, pts[j].1 - ry);
            let cross = x0 * y1 - x1 * y0;
            a2 += cross;
            cx += (x0 + x1) * cross;
            cy += (y0 + y1) * cross;
        }
        // ~1e-12 deg² ≈ a few cm² — below that the shoelace division is noise.
        if a2.abs() > 1e-12 {
            return (rx + cx / (3.0 * a2), ry + cy / (3.0 * a2));
        }
    }
    let n = pts.len().max(1) as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x, sy + y));
    (sx / n, sy / n)
}

/// Fold one non-ASCII char to its ASCII spelling. German umlauts get their proper digraphs (ä to ae,
/// ß to ss); the rest of Latin-1 Supplement and Latin Extended-A strips to the base letter. Anything
/// else — CJK, Cyrillic, Greek, emoji — is unmappable and answers `None`, and the caller turns it
/// into a word break rather than gluing neighbours together.
fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'Ä' => "Ae",
        'ä' => "ae",
        'Ö' => "Oe",
        'ö' => "oe",
        'Ü' => "Ue",
        'ü' => "ue",
        'ß' | 'ſ' => "ss",
        'Æ' => "AE",
        'æ' => "ae",
        'Œ' => "OE",
        'œ' => "oe",
        'Ĳ' => "IJ",
        'ĳ' => "ij",
        'Þ' => "Th",
        'þ' => "th",
        'Ð' | 'Đ' | 'Ď' => "D",
        'ð' | 'đ' | 'ď' => "d",
        'À'..='Å' | 'Ā' | 'Ă' | 'Ą' => "A",
        'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
        'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => "C",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'È'..='Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => "E",
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "G",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'Ĥ' | 'Ħ' => "H",
        'ĥ' | 'ħ' => "h",
        'Ì'..='Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => "I",
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'Ĵ' => "J",
        'ĵ' => "j",
        'Ķ' => "K",
        'ķ' | 'ĸ' => "k",
        'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => "L",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' | 'Ŋ' => "N",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ŋ' => "n",
        'Ò'..='Õ' | 'Ø' | 'Ō' | 'Ŏ' | 'Ő' => "O",
        'ò'..='õ' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'Ŕ' | 'Ŗ' | 'Ř' => "R",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'Ś' | 'Ŝ' | 'Ş' | 'Š' => "S",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'Ţ' | 'Ť' | 'Ŧ' => "T",
        'ţ' | 'ť' | 'ŧ' => "t",
        'Ù'..='Û' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "U",
        'ù'..='û' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'Ŵ' => "W",
        'ŵ' => "w",
        'Ý' | 'Ŷ' | 'Ÿ' => "Y",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'Ź' | 'Ż' | 'Ž' => "Z",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Normalize an OSM `name` for the record's fixed-width, printable-ASCII `Name` field, one byte per
/// char: ASCII-fold, replace anything unmappable with a word break, collapse whitespace, trim, and
/// cap at 24 bytes. Empty after all that gives `None`, and the device shows the subtype label.
///
/// The fold is a format constraint, not a font one: the device font renders Latin-1 and Latin
/// Extended-A for phone-supplied route and ride names, and only these fixed-width packed POI names
/// fold.
pub fn normalize_name(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len().min(28));
    let mut pending_space = false;
    let emit = |s: &str, out: &mut String, pending: &mut bool| {
        if *pending && !out.is_empty() {
            out.push(' ');
        }
        *pending = false;
        out.push_str(s);
    };
    for c in raw.chars() {
        match c {
            // Printable ASCII minus space; 0x7F (DEL) has no glyph, so it falls
            // through to the word-break arm with the controls.
            '!'..='~' => {
                let mut buf = [0u8; 1];
                emit(c.encode_utf8(&mut buf), &mut out, &mut pending_space);
            }
            _ => match fold_char(c) {
                Some(piece) => emit(piece, &mut out, &mut pending_space),
                // Space, controls, and unmappable scripts all become one break.
                None => pending_space = true,
            },
        }
    }
    // Byte cap: everything is ASCII by now, so bytes == chars; re-trim in case
    // the cut lands just after a space.
    out.truncate(24);
    while out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn dedupe(candidates: Vec<Poi>) -> (Vec<Poi>, usize) {
    let mut seen = std::collections::HashSet::new();
    let total = candidates.len();
    let kept: Vec<_> =
        candidates.into_iter().filter(|p| p.metadata.source.0 == 0 || seen.insert(p.metadata.source)).collect();
    let dropped = total - kept.len();
    (kept, dropped)
}

/// Resolve only explicit OSM node membership, never nearby geometry.
pub fn resolve_approaches(
    pois: &mut [Poi],
    ways: &[crate::nav::RoutableWay],
    profiles: &[crate::serialize::NavProfile],
) {
    let wanted: std::collections::HashSet<_> = pois.iter().flat_map(|p| p.access_nodes.iter().copied()).collect();
    let mut nodes: HashMap<i64, ((i32, i32), u8)> = HashMap::new();
    for way in ways {
        let mask = profiles.iter().enumerate().fold(0, |mask, (i, profile)| {
            if profile.highway[(way.kind & 31) as usize] != 0 && profile.surface[(way.kind >> 5) as usize] != 0 {
                mask | (1 << i)
            } else {
                mask
            }
        });
        for (&id, &coord) in way.node_ids.iter().zip(&way.coords) {
            if wanted.contains(&id) {
                nodes.entry(id).and_modify(|v| v.1 |= mask).or_insert((coord, mask));
            }
        }
    }
    for poi in pois {
        poi.metadata.approach = poi
            .access_nodes
            .iter()
            .filter_map(|id| {
                let &(coord, mask) = nodes.get(id)?;
                (mask != 0).then_some((*id, coord, mask))
            })
            .min_by_key(|(id, _, _)| *id)
            .map(|(id, (lon, lat), profile_mask)| obc_formats::obcm::PoiApproach {
                source: obc_formats::obcm::SourceId::osm(1, id as u64),
                lat,
                lon,
                profile_mask,
            });
    }
}

/// The pack-log line: per-category counts + how many dedup dropped, e.g.
/// `pois: water 312, campsite 41, … (dedup dropped 57)`.
pub fn format_counts(pois: &[Poi], dropped: usize) -> String {
    let mut counts = [0usize; CATEGORY_NAMES.len()];
    for p in pois {
        counts[table_row(p.subtype).category() as usize] += 1;
    }
    let per_cat: Vec<String> =
        (1..CATEGORY_NAMES.len()).map(|c| format!("{} {}", CATEGORY_NAMES[c], counts[c])).collect();
    format!("pois: {} (dedup dropped {dropped})", per_cat.join(", "))
}

/// `--dump-pois` output: one line per POI for eyeballing against a known extract.
pub fn dump(pois: &[Poi]) {
    for p in pois {
        let row = table_row(p.subtype);
        println!(
            "poi: {}/{} ({}) at {:.6},{:.6} name={:?}{} src={}",
            CATEGORY_NAMES[row.category() as usize],
            row.value,
            row.label(),
            p.lat_udeg as f64 / 1e6,
            p.lon_udeg as f64 / 1e6,
            p.name.as_deref().unwrap_or("-"),
            p.population.map_or(String::new(), |n| format!(" pop={n}")),
            if p.from_node { "node" } else { "way" },
        );
    }
}

/// `--dump-hours` output: one line per POI that carries a parsed schedule, for
/// eyeballing the parsed weekly hours against the raw `opening_hours` in an
/// extract. POIs without hours are skipped.
pub fn dump_hours(pois: &[Poi]) {
    for p in pois {
        if let Some(sched) = &p.hours {
            let name = p.name.as_deref().unwrap_or_else(|| table_row(p.subtype).label());
            println!("hours: {}: {}", name, crate::hours::describe(sched));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the packer's OSM-tag classification: subtype ids are normative and append-only, so any
    /// edit to an existing row must fail a test rather than slip through review. This also asserts
    /// that every subtype maps back to the expected category and label in `obc-formats`, so the two
    /// crates cannot drift.
    #[test]
    fn table_is_pinned() {
        // (subtype, key, value, expected category id, expected fallback label). The last two columns
        // are what `obc-formats` must return for this subtype.
        let expect: [(u8, &str, &str, u8, &str); 24] = [
            (1, "amenity", "drinking_water", 1, "Drinking water"),
            (2, "natural", "spring", 1, "Spring"),
            (3, "man_made", "water_tap", 1, "Water tap"),
            (4, "amenity", "water_point", 1, "Water point"),
            (5, "tourism", "camp_site", 2, "Campsite"),
            (6, "tourism", "caravan_site", 2, "Caravan site"),
            (7, "tourism", "hotel", 3, "Hotel"),
            (8, "tourism", "hostel", 3, "Hostel"),
            (9, "tourism", "guest_house", 3, "Guest house"),
            (10, "tourism", "motel", 3, "Motel"),
            (11, "tourism", "wilderness_hut", 3, "Wilderness hut"),
            (12, "tourism", "alpine_hut", 3, "Alpine hut"),
            (13, "shop", "supermarket", 4, "Supermarket"),
            (14, "shop", "convenience", 4, "Convenience"),
            (15, "shop", "bakery", 4, "Bakery"),
            (16, "amenity", "marketplace", 4, "Marketplace"),
            (17, "amenity", "pharmacy", 5, "Pharmacy"),
            (18, "shop", "bicycle", 6, "Bike shop"),
            (19, "natural", "peak", 7, "Summit"),
            (20, "railway", "station", 8, "Train station"),
            (21, "place", "city", 9, "City"),
            (22, "place", "town", 9, "Town"),
            (23, "place", "village", 9, "Village"),
            (24, "place", "hamlet", 9, "Hamlet"),
        ];
        assert_eq!(POI_TABLE.len(), expect.len(), "the pin covers every row; `zip` would skip the tail");
        for (row, &(sub, k, v, cat, label)) in POI_TABLE.iter().zip(expect.iter()) {
            assert_eq!((row.subtype, row.key, row.value), (sub, k, v), "packer classification pinned");
            // The derived shared category + label match the pinned expectation → no drift.
            assert_eq!((row.category(), row.label()), (cat, label), "obc-formats table agrees for subtype {sub}");
        }
        // Subtype ids are dense and 1-based (table_row indexes on that).
        for (i, row) in POI_TABLE.iter().enumerate() {
            assert_eq!(row.subtype as usize, i + 1);
        }
    }

    #[test]
    fn classify_first_match_wins_table_order() {
        // Both water rows present — drinking_water (row 0) beats spring (row 1),
        // regardless of tag iteration order.
        let fwd = [("amenity", "drinking_water"), ("natural", "spring")];
        let rev = [("natural", "spring"), ("amenity", "drinking_water")];
        assert_eq!(classify(fwd).unwrap().subtype, 1);
        assert_eq!(classify(rev).unwrap().subtype, 1);
        // Cross-category: supermarket (row 12) beats pharmacy (row 16).
        let mixed = [("amenity", "pharmacy"), ("shop", "supermarket")];
        assert_eq!(classify(mixed).unwrap().subtype, 13);
    }

    #[test]
    fn summit_names_heights_and_close_twins_are_preserved() {
        let Classification { subtype, name, raw_hours: hours, elevation_m: height, .. } =
            classify([("natural", "peak"), ("name", "Mönch"), ("ele", "4107.4 m"), ("opening_hours", "24/7")]).unwrap();
        assert_eq!((subtype, name.as_deref(), hours, height), (19, Some("Mönch"), None, Some(4107)));
        assert!(classify([("natural", "peak")]).is_none());
        let tags = |height| classify([("natural", "peak"), ("name", "Hill"), ("ele", height)]).unwrap().elevation_m;
        assert_eq!(tags("-25.7"), Some(-26));
        for value in ["NaN", "32768", "-32768", "unknown"] {
            assert_eq!(tags(value), None);
        }
        let name = classify([("natural", "peak"), ("name", "abcdefghijklmnopqrstuvwé")]).unwrap().name.unwrap();
        assert_eq!(name, "abcdefghijklmnopqrstuvw", "UTF-8 truncation keeps whole characters");
        let a = poi(19, 48.0, 7.8, Some("West summit"), true);
        let b = poi(19, 48.0001, 7.8, Some("East summit"), true);
        let (mut summits, dropped) = dedupe(vec![a.clone(), a, b]);
        assert_eq!((summits.len(), dropped), (2, 1), "distinct nearby summits are not merged");
        summits[0].elevation_m = Some(100);
        struct Terrain;
        impl obc_elevation::ElevationSource for Terrain {
            fn sample(&mut self, _: i32, _: i32) -> Option<i16> {
                Some(-20)
            }
        }
        fill_summit_elevations(&mut summits, &mut Terrain);
        assert_eq!(summits[0].elevation_m, Some(100), "OSM elevation wins");
        assert_eq!(summits[1].elevation_m, Some(-20), "missing elevation uses the DEM");
    }

    #[test]
    fn classify_no_match_and_name_capture() {
        assert_eq!(classify([("amenity", "parking"), ("name", "P1")]), None);
        assert_eq!(classify([("shop", "butcher")]), None);
        let Classification { subtype: sub, name, raw_hours: hours, .. } =
            classify([("name", "Alte Quelle"), ("natural", "spring")]).unwrap();
        assert_eq!((sub, name.as_deref(), hours), (2, Some("Alte Quelle"), None));
        // opening_hours captured raw alongside the match (parsed by the caller).
        let Classification { subtype: sub, raw_hours: hours, .. } =
            classify([("shop", "supermarket"), ("opening_hours", "Mo-Fr 08:00-18:00")]).unwrap();
        assert_eq!((sub, hours), (13, Some("Mo-Fr 08:00-18:00")));
        // Key and value must both match — near misses don't classify.
        assert_eq!(classify([("natural", "water")]), None);
        assert_eq!(classify([("building", "supermarket")]), None);
    }

    #[test]
    fn centroid_square_and_degenerate() {
        // Unit square with closing vertex — centroid dead center.
        let sq = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (0.0, 0.0)];
        assert_eq!(ring_centroid(&sq), (1.0, 1.0));
        // Winding order must not matter.
        let sq_cw = [(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0), (0.0, 0.0)];
        assert_eq!(ring_centroid(&sq_cw), (1.0, 1.0));
        // Degenerate: a collinear ring has zero area, so the vertex mean, with the closing vertex
        // excluded so the mean is not biased toward it.
        let line = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (0.0, 0.0)];
        let (cx, cy) = ring_centroid(&line);
        assert!((cx - 1.0).abs() < 1e-12 && cy == 0.0);
        // Two distinct vertices ⇒ mean of the two.
        let seg = [(0.0, 0.0), (4.0, 2.0), (0.0, 0.0)];
        assert_eq!(ring_centroid(&seg), (2.0, 1.0));
    }

    #[test]
    fn name_fold_umlauts_and_diacritics() {
        assert_eq!(normalize_name("Müller Bäckerei").as_deref(), Some("Mueller Baeckerei"));
        assert_eq!(normalize_name("Weißes Rößl").as_deref(), Some("Weisses Roessl"));
        assert_eq!(normalize_name("Café à l'Ouest").as_deref(), Some("Cafe a l'Ouest"));
        assert_eq!(normalize_name("Żabka Šumava").as_deref(), Some("Zabka Sumava"));
        assert_eq!(normalize_name("Señor Løkke").as_deref(), Some("Senor Lokke"));
    }

    #[test]
    fn name_unmappable_becomes_empty_or_break() {
        // Pure CJK ⇒ unnamed (device falls back to the subtype label).
        assert_eq!(normalize_name("北京烤鸭"), None);
        assert_eq!(normalize_name("Καφενείο"), None);
        // Mixed: the unmappable run breaks the word instead of gluing neighbors.
        assert_eq!(normalize_name("Edeka 市場 Nord").as_deref(), Some("Edeka Nord"));
        assert_eq!(normalize_name("AB水CD").as_deref(), Some("AB CD"));
        // Whitespace collapse + trim, control chars stripped.
        assert_eq!(normalize_name("  Zum  \t Hirschen ").as_deref(), Some("Zum Hirschen"));
        assert_eq!(normalize_name("\u{7f}\u{1}"), None);
    }

    #[test]
    fn name_truncates_at_24_bytes() {
        let exact = "123456789012345678901234"; // 24 bytes (the v7 Name field width)
        assert_eq!(normalize_name(exact).as_deref(), Some(exact));
        assert_eq!(normalize_name("1234567890123456789012345").as_deref(), Some(exact));
        // A cut landing right after a space must not leave a trailing space.
        assert_eq!(normalize_name("12345678901234567890123 X").as_deref(), Some("12345678901234567890123"));
        // Fold digraphs count toward the cap (bytes, not source chars): 12 × "ae" = 24 bytes.
        assert_eq!(normalize_name("ääääääääääää").as_deref(), Some("aeaeaeaeaeaeaeaeaeaeaeae"));
    }

    fn poi(subtype: u8, lat: f64, lon: f64, name: Option<&str>, from_node: bool) -> Poi {
        Poi {
            metadata: obc_formats::obcm::PoiMetadata {
                source: obc_formats::obcm::SourceId::osm(
                    1,
                    ((to_udeg(lat) as u32 as u64) << 20 | to_udeg(lon) as u32 as u64).max(1),
                ),
                approach: None,
            },
            access_nodes: Vec::new(),
            wikidata: None,
            wikipedia: None,
            subtype,
            lon_udeg: to_udeg(lon),
            lat_udeg: to_udeg(lat),
            name: name.map(String::from),
            from_node,
            hours: None,
            elevation_m: None,
            population: None,
        }
    }

    #[test]
    fn distinct_colocated_entities_survive() {
        use obc_formats::obcm::SourceId;
        let mut a = poi(1, 48.0, 7.8, Some("A"), true);
        let mut b = a.clone();
        a.metadata.source = SourceId::osm(1, 100);
        b.metadata.source = SourceId::osm(1, 101);
        let (kept, dropped) = dedupe(vec![a.clone(), b, a]);
        assert_eq!(kept.len(), 2);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn approach_requires_explicit_topology() {
        use crate::nav::RoutableWay;
        let mut linked = poi(1, 48.0, 7.8, Some("A"), true);
        linked.access_nodes = vec![10];
        let mut unrelated = linked.clone();
        unrelated.access_nodes = vec![11];
        let mut pois = vec![linked, unrelated];
        let ways = [RoutableWay {
            node_ids: vec![10, 20],
            coords: vec![(7_800_000, 48_000_000), (7_800_010, 48_000_000)],
            kind: 1,
        }];
        resolve_approaches(&mut pois, &ways, &crate::config::default_profiles());
        assert!(pois[0].metadata.approach.is_some());
        assert!(pois[1].metadata.approach.is_none());
    }

    #[test]
    fn counts_line_format() {
        let pois = vec![
            poi(1, 48.0, 7.8, None, true),
            poi(2, 48.1, 7.8, None, true),
            poi(5, 48.2, 7.8, None, false),
            poi(13, 48.3, 7.8, None, true),
        ];
        assert_eq!(
            format_counts(&pois, 3),
            "pois: water 2, campsite 1, accommodation 0, resupply 1, pharmacy 0, bike shop 0, summit 0, \
             train station 0, settlement 0 (dedup dropped 3)"
        );
    }

    /// One settlement tag set: the `place` value plus whatever else the case needs.
    fn place<'a>(value: &'a str, tags: &[(&'a str, &'a str)]) -> Option<Classification<'a>> {
        let mut all = vec![("place", value)];
        all.extend_from_slice(tags);
        classify(all)
    }

    #[test]
    fn place_tags_classify_to_settlement_subtypes() {
        for (value, subtype) in [("city", 21), ("town", 22), ("village", 23), ("hamlet", 24)] {
            let c = place(value, &[("name", "Ort")]).expect("a named place classifies");
            assert_eq!((c.subtype, c.name.as_deref()), (subtype, Some("Ort")));
        }
        assert_eq!(place("suburb", &[("name", "Ort")]), None, "only the four captured classes");
    }

    #[test]
    fn a_service_tag_wins_over_a_place_tag() {
        let c = place("village", &[("shop", "bakery"), ("name", "Baeckerdorf")]).expect("classifies");
        assert_eq!(c.subtype, 15, "the settlement rows sit last in table order");
    }

    #[test]
    fn a_settlement_keeps_its_diacritics() {
        let c = place("village", &[("name", "Grüßau")]).expect("classifies");
        assert_eq!(c.name.as_deref(), Some("Grüßau"), "the device font holds these, so nothing folds");
    }

    #[test]
    fn an_unshowable_name_folds_then_falls_back_to_english() {
        assert_eq!(place("village", &[("name", "Мирный")]), None, "no fold and no English name");
        let c = place("village", &[("name", "東京"), ("name:en", "Tokyo")]).expect("classifies");
        assert_eq!(c.name.as_deref(), Some("Tokyo"));
        let c = place("village", &[("name", "東京"), ("int_name", "Tokyo")]).expect("classifies");
        assert_eq!(c.name.as_deref(), Some("Tokyo"), "int_name is the last fall-back");
        // A name the fold can spell never reaches the language fall-backs.
        let c = place("village", &[("name", "Ost—Dorf"), ("name:en", "East")]).expect("classifies");
        assert_eq!(c.name.as_deref(), Some("Ost Dorf"), "the fold answers first");
    }

    #[test]
    fn a_shorter_short_name_wins() {
        let c = place("city", &[("name", "Freiburg im Breisgau"), ("short_name", "Freiburg")]).expect("classifies");
        assert_eq!(c.name.as_deref(), Some("Freiburg"), "the map shows 12 characters");
        // A short name that is not shorter, empty, or absent leaves the name alone.
        for short in ["Freiburg im Breisgau i. Br.", "  ", "Freiburg im Breisgau"] {
            let c = place("city", &[("name", "Freiburg im Breisgau"), ("short_name", short)]).expect("classifies");
            assert_eq!(c.name.as_deref(), Some("Freiburg im Breisgau"));
        }
        // The choice happens first; the fall-backs and the byte cut run on its result.
        let c = place("village", &[("name", "東京都"), ("short_name", "東京"), ("name:en", "Tokyo")]).expect("class");
        assert_eq!(c.name.as_deref(), Some("Tokyo"));
    }

    #[test]
    fn a_settlement_without_a_name_is_dropped() {
        assert_eq!(place("village", &[]), None);
        assert_eq!(place("village", &[("name", "  ")]), None);
    }

    #[test]
    fn a_long_settlement_name_is_cut_on_a_character_boundary() {
        let c = place("hamlet", &[("name", "A very long settlement name indeed")]).expect("classifies");
        let name = c.name.expect("named");
        assert_eq!((name.as_str(), name.len()), ("A very long settlement n", POI_NAME_LEN));
        let c = place("hamlet", &[("name", "abcdefghijklmnopqrstuvwé")]).expect("classifies");
        assert_eq!(c.name.as_deref(), Some("abcdefghijklmnopqrstuvw"), "a two-byte character does not split");
    }

    #[test]
    fn a_population_becomes_hundreds_and_saturates() {
        let payload = |tags: &[(&str, &str)]| settlement_payload(place("city", tags).expect("classifies").population);
        assert_eq!(payload(&[("name", "Testville"), ("population", "250000")]), 2500);
        assert_eq!(payload(&[("name", "Testville"), ("population", "900000000")]), SETTLEMENT_POPULATION_MAX);
        assert_eq!(payload(&[("name", "Testville")]), SETTLEMENT_POPULATION_UNKNOWN);
        assert_eq!(payload(&[("name", "Testville"), ("population", "about 900")]), SETTLEMENT_POPULATION_UNKNOWN);
        assert_eq!(payload(&[("name", "Testville"), ("population", "12 500")]), 125, "spaces are removed");
    }
}
