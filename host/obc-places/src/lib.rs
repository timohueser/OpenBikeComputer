//! OSM place classification and coordinates shared by host builders.

pub mod hours;
pub mod metadata;
pub mod name;
pub mod routing;

use obc_formats::obcm::{
    poi_directory_category_of, poi_label_of, SETTLEMENT_SUBTYPE_CITY, SETTLEMENT_SUBTYPE_HAMLET,
    SETTLEMENT_SUBTYPE_TOWN, SETTLEMENT_SUBTYPE_VILLAGE, SUMMIT_SUBTYPE_ID,
};

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

/// Match shared categories before planner-only categories.
pub fn classify<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<&'static PoiKind> {
    let mut best = None;
    for (key, value) in tags {
        for (i, kind) in POI_TABLE.iter().enumerate().take(best.unwrap_or(POI_TABLE.len())) {
            if kind.key == key && kind.value == value {
                best = Some(i);
                break;
            }
        }
    }
    best.map(|i| &POI_TABLE[i])
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

/// Use the largest outer ring, with a coordinate tie-break independent of member order.
pub fn area_center<'a>(outers: impl IntoIterator<Item = &'a [(f64, f64)]>) -> Option<(f64, f64)> {
    outers
        .into_iter()
        .filter(|ring| !ring.is_empty())
        .map(|ring| {
            let (x, y) = ring[0];
            let area =
                ring.windows(2).map(|p| (p[0].0 - x) * (p[1].1 - y) - (p[1].0 - x) * (p[0].1 - y)).sum::<f64>().abs();
            (area, ring_centroid(ring))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1 .0.total_cmp(&b.1 .0)).then(a.1 .1.total_cmp(&b.1 .1)))
        .map(|(_, point)| point)
}
