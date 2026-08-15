//! `poi.rs` — OSM point-of-interest extraction: classify nodes and closed ways
//! against the fixed category/subtype table, normalize names for the device font,
//! and collapse OSM double-mapping (issue #422, epic #115).
//!
//! The table below is **canonical and append-only**: ids are stable from day one,
//! will be mirrored in firmware, and pinned normatively in `OBCM_Spec.md` by the
//! format sub-issue (#423). Subtype `0` is reserved; `0xFF` is reserved as the
//! end-of-chunk sentinel (mirrors the style-id sentinel). First match in table
//! order wins, the same convention as the config's style map.
//!
//! This stage is deliberately config-free (locked decision on #115): the tag
//! mapping is hardcoded, so packing the same extract always yields the same POIs.

use std::collections::HashMap;

use obc_formats::obcm::{poi_category_of, poi_label_of};
use obc_map_scene::M_PER_DEG;

use crate::hours::Schedule;

/// One row of the canonical table: the OSM `key=value` classification and the
/// subtype id it maps to. The subtype's **category** and **fallback label** are
/// *not* stored here — they live once in `obc-formats` (the firmware source of truth beneath
/// `OBCM_Spec.md` §7.4), and this row derives them via
/// [`PoiKind::category`] / [`PoiKind::label`], so the category/label mapping is never
/// maintained in two places. Only the OSM tag classification (which the device never
/// needs) stays packer-side.
pub struct PoiKind {
    pub subtype: u8,
    pub key: &'static str,
    pub value: &'static str,
}

impl PoiKind {
    /// The category id (spec §7.4) this subtype belongs to, derived from `obc-formats`' canonical
    /// table. Every `POI_TABLE` subtype is valid there, so the unwrap never trips (the pinning test
    /// guarantees it for every row).
    pub fn category(&self) -> u8 {
        poi_category_of(self.subtype).expect("POI_TABLE subtype is in obc-formats' canonical table").id()
    }

    /// The device fallback label for this subtype, from `obc-formats`' canonical table (shown when
    /// OSM has no usable name). Valid for every `POI_TABLE` subtype (see the pinning test).
    pub fn label(&self) -> &'static str {
        poi_label_of(self.subtype).expect("POI_TABLE subtype is in obc-formats' canonical table")
    }
}

const fn kind(subtype: u8, key: &'static str, value: &'static str) -> PoiKind {
    PoiKind { subtype, key, value }
}

/// The canonical OSM-tag → subtype classification (normative subtype ids —
/// append-only, never renumber). The subtype→category/label half of the table lives
/// in `obc-formats` (spec §7.4); this half is the OSM tag mapping the
/// packer owns. First match in table order wins (see [`classify`]).
pub const POI_TABLE: [PoiKind; 18] = [
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
];

/// Category display names for the pack log, indexed by category id (0 unused).
pub const CATEGORY_NAMES: [&str; 7] = ["", "water", "campsite", "accommodation", "resupply", "pharmacy", "bike shop"];

/// A classified POI candidate. Coordinates are µdeg (`round(deg * 1e6)`), the
/// same grid the serializer's chunk coords live on.
#[derive(Debug, Clone, PartialEq)]
pub struct Poi {
    pub subtype: u8,
    pub lon_udeg: i32,
    pub lat_udeg: i32,
    /// Normalized (ASCII-folded, ≤ 24 bytes) — `None` shows the subtype label.
    pub name: Option<String>,
    /// Nodes mark entrances; way-centroids are derived. Drives dedup priority.
    pub from_node: bool,
    /// Parsed weekly schedule from the OSM `opening_hours` tag, or `None` when the
    /// POI has no (parseable) hours. In-memory only in P1 (#440) — P2 pools these
    /// and stores a `hours_ref` on the POI record.
    pub hours: Option<Schedule>,
}

/// Look up a subtype's table row (subtype ids are 1-based and dense).
pub fn table_row(subtype: u8) -> &'static PoiKind {
    &POI_TABLE[subtype as usize - 1]
}

/// Classify a tag set against [`POI_TABLE`] — first match in **table order**
/// wins — and pull the normalized `name` plus the **raw** `opening_hours` value
/// alongside. One pass over the tags, no allocation on the (overwhelmingly common)
/// no-match path. The `opening_hours` string is returned unparsed (a borrowed
/// slice) so the fast path stays alloc-free; the caller parses it into a
/// [`Schedule`] via [`crate::hours::parse`] only on a match.
pub fn classify<'a, I>(tags: I) -> Option<(u8, Option<String>, Option<&'a str>)>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut best: Option<usize> = None;
    let mut raw_name: Option<&str> = None;
    let mut raw_hours: Option<&str> = None;
    for (k, v) in tags {
        if k == "name" {
            raw_name = Some(v);
            continue;
        }
        if k == "opening_hours" {
            raw_hours = Some(v);
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
    best.map(|i| (POI_TABLE[i].subtype, raw_name.and_then(normalize_name), raw_hours))
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

/// Fold one non-ASCII char to its ASCII spelling. German umlauts get their
/// proper digraphs (ä→ae, ß→ss — the taste call from #422, tested); the rest of
/// Latin-1 Supplement + Latin Extended-A strips to the base letter. Anything
/// else (CJK, Cyrillic, Greek, emoji) is unmappable ⇒ `None`, and the caller
/// turns it into a word break rather than gluing neighbors together.
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

/// Normalize an OSM `name` for the OBCM record's fixed-width, printable-ASCII
/// `Name` field (`0x20..=0x7E`, one byte per char): ASCII-fold, replace anything
/// unmappable with a word break, collapse whitespace, trim, cap at **24 bytes**
/// (the v7 record's widened `Name` field). Empty after all that ⇒ `None` (device
/// shows the subtype label). Note this fold is a *format* constraint, not a font
/// one — the device font renders Latin-1/Latin Extended-A for phone-supplied
/// route & ride names (see `obc-render/src/font_data.rs`); only these fixed-width
/// packed POI names fold.
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

/// Dedup radius: OSM double-mapping (node inside a same-tagged building way,
/// campsite area + entrance node) lands well inside this; genuinely distinct
/// same-category POIs are almost always farther apart. Accepted v1 risk: two
/// adjacent bakeries on one square merge.
const DEDUP_RADIUS_M: f64 = 50.0;

/// Equirectangular meters for the dedup grid — exact enough at 50 m scales.
fn meters(p: &Poi) -> (f64, f64) {
    let lat_deg = p.lat_udeg as f64 / 1e6;
    let x = (p.lon_udeg as f64 / 1e6) * M_PER_DEG * lat_deg.to_radians().cos();
    (x, lat_deg * M_PER_DEG)
}

/// Collapse duplicates: two candidates of the **same category** within
/// [`DEDUP_RADIUS_M`] are one POI. Keep by priority — node beats way-centroid,
/// then named beats unnamed, then first-seen. O(n) via a 50 m grid hash with a
/// 3×3 neighborhood check. Returns `(kept, dropped_count)`.
pub fn dedupe(mut candidates: Vec<Poi>) -> (Vec<Poi>, usize) {
    // Stable sort = first-seen wins among equals; better candidates insert
    // first, so any later in-radius duplicate loses to the best of its cluster.
    candidates.sort_by_key(|p| (!p.from_node, p.name.is_none()));
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut kept: Vec<Poi> = Vec::new();
    let mut dropped = 0usize;
    'cand: for p in candidates {
        let (x, y) = meters(&p);
        let (cx, cy) = ((x / DEDUP_RADIUS_M).floor() as i64, (y / DEDUP_RADIUS_M).floor() as i64);
        let cat = table_row(p.subtype).category();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &i in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let q = &kept[i];
                    if table_row(q.subtype).category() != cat {
                        continue;
                    }
                    let (qx, qy) = meters(q);
                    if (x - qx).hypot(y - qy) < DEDUP_RADIUS_M {
                        dropped += 1;
                        continue 'cand;
                    }
                }
            }
        }
        grid.entry((cx, cy)).or_default().push(kept.len());
        kept.push(p);
    }
    (kept, dropped)
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
            "poi: {}/{} ({}) at {:.6},{:.6} name={:?} src={}",
            CATEGORY_NAMES[row.category() as usize],
            row.value,
            row.label(),
            p.lat_udeg as f64 / 1e6,
            p.lon_udeg as f64 / 1e6,
            p.name.as_deref().unwrap_or("-"),
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

    /// Pin the packer's OSM-tag classification — subtype ids are normative and
    /// append-only, so any edit to an existing row must fail a test, not slip through
    /// review. The category/label half of each row lives in `obc-formats`' canonical
    /// table; this test also asserts every subtype maps back to the **expected**
    /// category + label there, so the two crates can't drift.
    #[test]
    fn table_is_pinned() {
        // (subtype, key, value, expected category id, expected fallback label). The category + label
        // columns are what `obc-formats` must return for this subtype — the cross-crate guard.
        let expect: [(u8, &str, &str, u8, &str); 18] = [
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
        ];
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
        assert_eq!(classify(fwd).unwrap().0, 1);
        assert_eq!(classify(rev).unwrap().0, 1);
        // Cross-category: supermarket (row 12) beats pharmacy (row 16).
        let mixed = [("amenity", "pharmacy"), ("shop", "supermarket")];
        assert_eq!(classify(mixed).unwrap().0, 13);
    }

    #[test]
    fn classify_no_match_and_name_capture() {
        assert_eq!(classify([("amenity", "parking"), ("name", "P1")]), None);
        assert_eq!(classify([("shop", "butcher")]), None);
        let (sub, name, hours) = classify([("name", "Alte Quelle"), ("natural", "spring")]).unwrap();
        assert_eq!((sub, name.as_deref(), hours), (2, Some("Alte Quelle"), None));
        // opening_hours captured raw alongside the match (parsed by the caller).
        let (sub, _, hours) = classify([("shop", "supermarket"), ("opening_hours", "Mo-Fr 08:00-18:00")]).unwrap();
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
        // Degenerate: collinear ring has zero area ⇒ vertex mean (closing
        // vertex excluded, so the mean is not biased toward it).
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
            subtype,
            lon_udeg: to_udeg(lon),
            lat_udeg: to_udeg(lat),
            name: name.map(String::from),
            from_node,
            hours: None,
        }
    }

    #[test]
    fn dedupe_priority_node_beats_way_beats_unnamed() {
        // ~28 m apart (0.00025° lat at equator scale). Way-centroid is named,
        // node is not — node STILL wins (position beats name).
        let way = poi(13, 48.0, 7.8, Some("Edeka"), false);
        let node = poi(13, 48.00025, 7.8, None, true);
        let (kept, dropped) = dedupe(vec![way.clone(), node.clone()]);
        assert_eq!((kept.len(), dropped), (1, 1));
        assert_eq!(kept[0], node);
        // Among two nodes, named beats unnamed…
        let unnamed = poi(1, 48.0, 7.8, None, true);
        let named = poi(1, 48.00025, 7.8, Some("Brunnen"), true);
        let (kept, _) = dedupe(vec![unnamed, named.clone()]);
        assert_eq!(kept, vec![named.clone()]);
        // …and among equals, first-seen wins.
        let first = poi(1, 48.0, 7.8, Some("A"), true);
        let (kept, _) = dedupe(vec![first.clone(), named]);
        assert_eq!(kept, vec![first]);
    }

    #[test]
    fn dedupe_radius_and_category_boundaries() {
        // 40 m apart ⇒ merged; 60 m apart ⇒ both kept (µdeg per meter of
        // latitude: 1e6 / 111320 ≈ 8.98).
        let a = poi(1, 48.0, 7.8, Some("A"), true);
        let near = poi(1, 48.0 + 40.0 / M_PER_DEG, 7.8, Some("B"), true);
        let far = poi(1, 48.0 + 60.0 / M_PER_DEG, 7.8, Some("C"), true);
        assert_eq!(dedupe(vec![a.clone(), near]).1, 1);
        assert_eq!(dedupe(vec![a.clone(), far]).1, 0);
        // Same spot, different category ⇒ never a duplicate.
        let pharmacy = poi(17, 48.0, 7.8, Some("A"), true);
        let bakery = poi(15, 48.0, 7.8, Some("A"), true);
        assert_eq!(dedupe(vec![pharmacy, bakery]).1, 0);
        // Same category, different subtype (spring vs drinking_water) DOES merge.
        let spring = poi(2, 48.0, 7.8, None, true);
        assert_eq!(dedupe(vec![a, spring]).1, 1);
    }

    #[test]
    fn dedupe_across_grid_cell_edge() {
        // Two points ~45 m apart placed to straddle a 50 m grid boundary — the
        // 3×3 neighborhood must still find the pair.
        let lat0 = (50.0 * 179.0) / M_PER_DEG; // just below a cell edge
        let a = poi(5, lat0, 7.8, Some("Camp"), true);
        let b = poi(5, lat0 + 45.0 / M_PER_DEG, 7.8, None, true);
        let (kept, dropped) = dedupe(vec![a.clone(), b]);
        assert_eq!((kept.len(), dropped), (1, 1));
        assert_eq!(kept[0], a);
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
            "pois: water 2, campsite 1, accommodation 0, resupply 1, pharmacy 0, bike shop 0 (dedup dropped 3)"
        );
    }
}
