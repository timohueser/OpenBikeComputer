//! Settlement names on the map: a bounded candidate cache and the greedy label pass.
//!
//! The cache is refilled outside the draw path, over a viewport padded by [`CANDIDATE_PAD_PX`], and
//! only when the map, the scale band or the padded region stops covering the view. A frame then
//! orders the held candidates by [`rank`] and offers them to the shared
//! [`PointPlacement`](crate::screen::map::placement::PointPlacement), best first, until
//! [`MAX_LABELS`] are placed.
//!
//! [`rank`] holds no camera value, which is the whole stability mechanism: a label that is drawn in
//! one frame is drawn in the next one unless it leaves the panel, unless its class drops below its
//! scale limit, or unless a better settlement takes its space. No history and no timer are needed.

use core::cmp::Reverse;

use embedded_graphics::prelude::Point;
use heapless::Vec;
use obc_formats::obcm::{SettlementClass, POI_NAME_LEN};
use obc_map_scene::BBox;
use obc_reader::{Reader, Settlement};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface, Viewport,
};

use crate::screen::map::halo_text;
use crate::screen::map::placement::PointPlacement;
use crate::screen::palette::{INK, PARCHMENT};
use crate::screen::vocab::marquee::{fit, Fitted};

/// Candidates held over the padded viewport, at 44 bytes each. Nearly three times the label
/// budget, so a name refused for space gives way to the next one rather than to nothing, while the
/// whole cache stays inside the device's residual stack margin (see `resource_baseline.json`).
const MAX_CANDIDATES: usize = 16;
/// Labels drawn in one frame. One constant for every scale.
const MAX_LABELS: usize = 6;
/// Clear space around a label, in pixels. One constant for every scale.
const LABEL_MARGIN_PX: i32 = 12;
/// Characters shown. 12 at the 12 by 24 face is 144 pixels, 60 percent of the panel.
const MAX_LABEL_CHARS: usize = 12;
/// Slack around the panel, so a small pan needs no new query.
const CANDIDATE_PAD_PX: f32 = 48.0;
/// Gap between the place point and the top of its name.
const LABEL_ANCHOR_DY: i32 = 3;

/// The coarsest scale, in metres per pixel, at which each class still shows a name. 180, 50 and 16
/// are tiers of the shipped level-of-detail ladder.
const CITY_MAX_MPP: f32 = 600.0;
const TOWN_MAX_MPP: f32 = 180.0;
const VILLAGE_MAX_MPP: f32 = 50.0;
const HAMLET_MAX_MPP: f32 = 16.0;
/// Below this scale the rider is inside the settlement and the name only covers the roads.
const SETTLEMENT_MIN_MPP: f32 = 2.0;

/// The classes that show a name at `mpp`, as a bit for each [`SettlementClass`] discriminant. Zero
/// means the overlay is quiet: nothing is queried and nothing is drawn.
fn class_mask(mpp: f32) -> u8 {
    if !(mpp.is_finite() && mpp >= SETTLEMENT_MIN_MPP) {
        return 0;
    }
    let mut mask = 0;
    for (class, limit) in [
        (SettlementClass::City, CITY_MAX_MPP),
        (SettlementClass::Town, TOWN_MAX_MPP),
        (SettlementClass::Village, VILLAGE_MAX_MPP),
        (SettlementClass::Hamlet, HAMLET_MAX_MPP),
    ] {
        if mpp <= limit {
            mask |= 1 << class as u8;
        }
    }
    mask
}

/// One held settlement. The reader's record without its source identity, which the overlay never
/// reads: 44 bytes on the device.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Candidate {
    lat: i32,
    lon: i32,
    /// People. Zero also stands for "the map holds no value"; [`rank`] orders the two the same way.
    population: u32,
    name: heapless::String<POI_NAME_LEN>,
    class: SettlementClass,
}

impl From<Settlement> for Candidate {
    fn from(s: Settlement) -> Self {
        Candidate { lat: s.lat, lon: s.lon, population: s.population.unwrap_or(0), name: s.name, class: s.class }
    }
}

/// The priority order, smallest first: class, then the larger population, then the shorter name,
/// then the position. It holds no camera value, so it does not change while the rider moves, and
/// the position tail makes the same map give the same order every time.
fn rank(c: &Candidate) -> (u8, Reverse<u32>, u8, i32, i32) {
    (c.class as u8, Reverse(c.population), c.name.chars().count() as u8, c.lat, c.lon)
}

/// What a held candidate set is *for*. A set taken for the same map and the same scale band is the
/// same set wherever the camera sits inside [`SettlementCache::region`].
#[derive(Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    generation: u32,
    class_mask: u8,
}

/// The [`App`](crate::App)-owned settlement candidates. One buffer, refilled from
/// [`prepare`](SettlementCache::prepare) outside the draw path; the draw pass only reads it.
pub(crate) struct SettlementCache {
    /// The key the held set was taken for. `None` when nothing is held, so "queried, nothing here"
    /// is distinguishable from "not queried yet".
    taken_for: Option<CacheKey>,
    /// The padded region the set covers. A view inside it needs no new query.
    region: BBox,
    items: Vec<Candidate, MAX_CANDIDATES>,
}

impl SettlementCache {
    pub(crate) const fn new() -> Self {
        SettlementCache {
            taken_for: None,
            region: BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            items: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Refill the candidates for `vp` when the held set no longer answers for it.
    ///
    /// `enabled` is the overlay's one switch input; it is always on today. A read failure is
    /// recorded like a successful query, so a map the reader cannot walk is asked once for each
    /// region rather than once per frame.
    pub(crate) fn prepare(&mut self, reader: Option<&Reader>, vp: &Viewport, enabled: bool) {
        let mask = if enabled { class_mask(vp.meters_per_pixel()) } else { 0 };
        let Some(reader) = reader.filter(|_| mask != 0) else {
            self.taken_for = None;
            self.items.clear();
            return;
        };
        let key = CacheKey { generation: reader.generation(), class_mask: mask };
        if self.taken_for == Some(key) && self.region.contains(&vp.visible_bbox()) {
            return;
        }
        let region = padded_bbox(vp);
        self.region = region;
        self.items.clear();
        let items = &mut self.items;
        let _ = reader.visit_settlements_in(&region, |s| {
            if mask & (1 << s.class as u8) != 0 {
                keep(items, s.into());
            }
        });
        self.taken_for = Some(key);
    }
}

/// Keep the best [`MAX_CANDIDATES`]: a full set gives its worst slot to a better arrival.
fn keep(items: &mut Vec<Candidate, MAX_CANDIDATES>, new: Candidate) {
    let Err(new) = items.push(new) else { return };
    let Some((i, worst)) = items.iter().enumerate().max_by_key(|(_, c)| rank(c)).map(|(i, c)| (i, rank(c))) else {
        return;
    };
    if rank(&new) < worst {
        items[i] = new;
    }
}

/// The map box covering the panel grown by [`CANDIDATE_PAD_PX`] on every side. All four padded
/// corners go in, so a rotated camera is covered for the same reason as
/// [`Viewport::visible_bbox`].
fn padded_bbox(vp: &Viewport) -> BBox {
    let (p, w, h) = (CANDIDATE_PAD_PX, vp.w, vp.h);
    let corners = [vp.to_map(-p, -p), vp.to_map(w + p, -p), vp.to_map(-p, h + p), vp.to_map(w + p, h + p)];
    let mut region = BBox { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN };
    for (lon, lat) in corners {
        region.min_lon = region.min_lon.min(lon);
        region.max_lon = region.max_lon.max(lon);
        region.min_lat = region.min_lat.min(lat);
        region.max_lat = region.max_lat.max(lat);
    }
    region
}

/// The name a label shows: [`MAX_LABEL_CHARS`] characters, cut with `..`.
fn label_of(c: &Candidate) -> Fitted {
    fit(c.name.as_str(), MAX_LABEL_CHARS)
}

/// Draw the settlement names, best first, into whatever space `place` still has. Text is drawn
/// through the shared halo, and never rotated: a name reads upright at every heading.
pub(crate) fn draw_labels(cv: &mut impl Surface, vp: &Viewport, cache: &SettlementCache, place: &mut PointPlacement) {
    for (at, i) in placements(vp, cache, place) {
        halo_text(cv, &label_of(&cache.items[usize::from(i)]), at, Font::Label, TextAlign::Center, INK, PARCHMENT);
    }
}

/// The labels this frame draws, as `(anchor, candidate index)` in the order they were placed.
///
/// A name that does not fit is dropped, never moved: `place` refuses a box that crosses the panel
/// bounds, sits on reserved chrome, or comes within [`LABEL_MARGIN_PX`] of a name already placed.
/// A class past its scale limit for *this* camera is skipped, whatever the cache holds.
fn placements(vp: &Viewport, cache: &SettlementCache, place: &mut PointPlacement) -> Vec<(Point, u8), MAX_LABELS> {
    // The scale limits answer to the camera that draws, which is not always the one the cache was
    // filled for: the browse screens fit a whole route into the panel at a much coarser scale.
    let mask = class_mask(vp.meters_per_pixel());
    // Sort small indices, so the held set keeps the order the query gave it.
    let mut order: Vec<u8, MAX_CANDIDATES> = (0..cache.items.len() as u8).collect();
    order.sort_unstable_by_key(|&i| rank(&cache.items[usize::from(i)]));

    let mut placed = Vec::new();
    for i in order {
        if placed.is_full() {
            break;
        }
        let c = &cache.items[usize::from(i)];
        if mask & (1 << c.class as u8) == 0 {
            continue;
        }
        let (x, y) = vp.to_screen(c.lon, c.lat);
        let at = Point::new(x, y + LABEL_ANCHOR_DY);
        let tw = text_width(&label_of(c), Font::Label) as i32;
        let th = Font::Label.line_height() as i32;
        if place.try_place(rect(at.x - tw / 2, at.y, tw, th), LABEL_MARGIN_PX) {
            let _ = placed.push((at, i));
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use obc_formats::obcm::SETTLEMENT_POPULATION_UNKNOWN;
    use obc_reader::{ByteSource, MapCache, MapTables, SliceSource};
    use obcm_testkit::{build_poi_map, PoiSpec};
    use std::{string::String, vec, vec::Vec as StdVec};

    const BBOX: (i32, i32, i32, i32) = (7_000_000, 47_000_000, 8_000_000, 48_000_000);
    const CAM: (i32, i32) = (7_500_000, 47_500_000);
    const PANEL: (f32, f32) = (240.0, 320.0);

    /// A counting byte source, so a test can prove that a frame ran no query at all.
    struct CountSource<'a> {
        bytes: &'a [u8],
        reads: Cell<u32>,
    }
    impl ByteSource for CountSource<'_> {
        fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
            self.reads.set(self.reads.get() + 1);
            SliceSource(self.bytes).read_at(off, buf)
        }
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
    }

    fn vp_at(cam: (i32, i32), mpp: f32) -> Viewport {
        Viewport::new(PANEL.0, PANEL.1, cam.0, cam.1, obc_render::zoom_for_mpp(mpp))
    }

    fn candidate(class: SettlementClass, name: &str, population: u32, lat: i32, lon: i32) -> Candidate {
        Candidate { lat, lon, population, name: heapless::String::try_from(name).expect("a test name fits"), class }
    }

    /// A cache holding `items`, as a refill over the whole map would leave it.
    fn cache_of(items: &[Candidate]) -> SettlementCache {
        let mut cache = SettlementCache::new();
        cache.taken_for = Some(CacheKey { generation: 0, class_mask: 0b1111 });
        cache.region = BBox { min_lon: BBOX.0, min_lat: BBOX.1, max_lon: BBOX.2, max_lat: BBOX.3 };
        for item in items {
            cache.items.push(item.clone()).expect("the test set fits");
        }
        cache
    }

    /// A placer over the whole panel with no reserved chrome.
    fn open_panel() -> PointPlacement {
        PointPlacement::new(rect(0, 0, PANEL.0 as i32, PANEL.1 as i32), &[])
    }

    /// The names a frame draws, in placement order.
    fn drawn(vp: &Viewport, cache: &SettlementCache, place: &mut PointPlacement) -> StdVec<String> {
        placements(vp, cache, place)
            .into_iter()
            .map(|(_, i)| label_of(&cache.items[usize::from(i)]).as_str().into())
            .collect()
    }

    /// A settlement at the panel point `(x, y)` of `vp`.
    fn at_screen(vp: &Viewport, x: f32, y: f32, class: SettlementClass, name: &str, population: u32) -> Candidate {
        let (lon, lat) = vp.to_map(x, y);
        candidate(class, name, population, lat, lon)
    }

    fn spec(lat: i32, lon: i32, class: SettlementClass, name: &str, payload: u16) -> PoiSpec {
        PoiSpec { lat, lon, subtype: class.subtype_id(), name: name.into(), payload }
    }

    #[test]
    fn the_class_mask_follows_the_scale() {
        assert_eq!(class_mask(400.0), 0b0001, "only cities at 400 m/px");
        assert_eq!(class_mask(100.0), 0b0011, "towns join at 100 m/px");
        assert_eq!(class_mask(30.0), 0b0111, "villages join at 30 m/px");
        assert_eq!(class_mask(10.0), 0b1111, "hamlets join at 10 m/px");
        assert_eq!(class_mask(SETTLEMENT_MIN_MPP), 0b1111, "the riding end of the band still shows every class");
        assert_eq!(class_mask(1.0), 0, "inside a settlement the names only cover the roads");
        assert_eq!(class_mask(700.0), 0, "past the city limit nothing is named");

        // The locked edges themselves: a changed constant has to fail here.
        for (limit, at_limit, above) in [
            (CITY_MAX_MPP, 0b0001, 0b0000),
            (TOWN_MAX_MPP, 0b0011, 0b0001),
            (VILLAGE_MAX_MPP, 0b0111, 0b0011),
            (HAMLET_MAX_MPP, 0b1111, 0b0111),
        ] {
            assert_eq!(class_mask(limit), at_limit, "the class still shows a name at {limit} m/px");
            assert_eq!(class_mask(limit * 1.01), above, "just past {limit} m/px it is gone");
        }
        for degenerate in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            assert_eq!(class_mask(degenerate), 0, "{degenerate} is not a scale");
        }
    }

    #[test]
    fn the_rank_orders_by_class_then_population_then_name_then_position() {
        let city = candidate(SettlementClass::City, "A", 1, 0, 0);
        let hamlet = candidate(SettlementClass::Hamlet, "A", 900_000, 0, 0);
        assert!(rank(&city) < rank(&hamlet), "class comes before population");

        let big = candidate(SettlementClass::Town, "Bbbb", 20_000, 0, 0);
        let small = candidate(SettlementClass::Town, "A", 5_000, 0, 0);
        assert!(rank(&big) < rank(&small), "the larger population wins inside a class");

        let short = candidate(SettlementClass::Town, "Buchenbach", 0, 0, 0);
        let long = candidate(SettlementClass::Town, "Sankt Peter im Tal", 0, 0, 0);
        assert!(rank(&short) < rank(&long), "an equal population gives the shorter name the space");

        let south = candidate(SettlementClass::Town, "Aa", 0, 10, 0);
        let north = candidate(SettlementClass::Town, "Aa", 0, 20, 0);
        assert!(rank(&south) < rank(&north), "the position is the last tie break");
        assert_eq!(rank(&south), rank(&candidate(SettlementClass::Town, "Aa", 0, 10, 0)), "the order is a function");
    }

    /// An unknown population ranks with a population of zero, which is what the overlay wants: a
    /// named place the map cannot size loses to one it can.
    #[test]
    fn an_unknown_population_ranks_last_inside_its_class() {
        let known = Candidate::from(Settlement {
            source: obc_formats::obcm::SourceId(1),
            lat: 0,
            lon: 0,
            class: SettlementClass::Village,
            name: heapless::String::try_from("Known").unwrap(),
            population: Some(1_200),
        });
        let unknown = Candidate::from(Settlement {
            source: obc_formats::obcm::SourceId(2),
            lat: 0,
            lon: 0,
            class: SettlementClass::Village,
            name: heapless::String::try_from("Unknwn").unwrap(),
            population: None,
        });
        assert_eq!(unknown.population, 0);
        assert!(rank(&known) < rank(&unknown));
    }

    // ---- the cache ----

    fn freiburg_map() -> StdVec<u8> {
        build_poi_map(
            BBOX,
            512,
            &[(
                obc_formats::obcm::SETTLEMENT_CATEGORY_ID,
                vec![
                    spec(47_500_000, 7_500_000, SettlementClass::City, "Freiburg", 2_300),
                    spec(47_508_000, 7_505_000, SettlementClass::Town, "Emmendingen", 280),
                    spec(47_492_000, 7_495_000, SettlementClass::Village, "Denzlingen", 130),
                    spec(47_488_000, 7_508_000, SettlementClass::Hamlet, "Hofsgrund", SETTLEMENT_POPULATION_UNKNOWN),
                ],
            )],
        )
    }

    /// The reader, its tables and the counting source, kept alive together for a cache test.
    macro_rules! reader_over {
        ($bytes:expr, $source:ident, $reader:ident) => {
            let $source = CountSource { bytes: &$bytes, reads: Cell::new(0) };
            let tables = MapTables::parse(&$source).unwrap();
            let cache = MapCache::new();
            let $reader = Reader::new(&$source, &tables, &cache);
        };
    }

    #[test]
    fn the_cache_holds_while_the_padded_region_covers_the_view() {
        let bytes = freiburg_map();
        reader_over!(bytes, source, reader);
        let vp = vp_at(CAM, 10.0);
        let mut cache = SettlementCache::new();
        cache.prepare(Some(&reader), &vp, true);
        assert_eq!(cache.items.len(), 4, "every class is inside the padded view at 10 m/px");
        let region = cache.region;
        let reads = source.reads.get();

        let (lon, lat) = vp.to_map(PANEL.0 / 2.0 + 10.0, PANEL.1 / 2.0);
        cache.prepare(Some(&reader), &vp_at((lon, lat), 10.0), true);
        assert_eq!(source.reads.get(), reads, "a 10 px pan stays inside the padded region and reads nothing");
        assert_eq!(cache.region, region, "the held region is untouched");

        let (lon, lat) = vp.to_map(PANEL.0 / 2.0 + 200.0, PANEL.1 / 2.0);
        cache.prepare(Some(&reader), &vp_at((lon, lat), 10.0), true);
        assert!(source.reads.get() > reads, "a 200 px pan leaves the padded region and refills");
        assert_ne!(cache.region, region);
    }

    #[test]
    fn a_scale_change_that_crosses_a_class_limit_refills_the_cache() {
        let bytes = freiburg_map();
        reader_over!(bytes, source, reader);
        let mut cache = SettlementCache::new();
        cache.prepare(Some(&reader), &vp_at(CAM, 10.0), true);
        assert_eq!(cache.items.len(), 4);
        let reads = source.reads.get();

        // 12 m/px is inside the hamlet band, and the padded region still covers the wider view.
        cache.prepare(Some(&reader), &vp_at(CAM, 12.0), true);
        assert_eq!(source.reads.get(), reads, "a scale change inside one band reuses the set");

        cache.prepare(Some(&reader), &vp_at(CAM, 20.0), true);
        assert!(source.reads.get() > reads, "crossing the hamlet limit refills");
        let names: StdVec<_> = cache.items.iter().map(|c| c.name.as_str()).collect();
        assert!(!names.contains(&"Hofsgrund"), "the hamlet is out of the band");
        assert!(names.contains(&"Denzlingen") && names.contains(&"Freiburg"));
    }

    #[test]
    fn nothing_is_read_when_the_overlay_is_off_or_the_scale_is_outside_every_band() {
        let bytes = freiburg_map();
        reader_over!(bytes, source, reader);
        let mut cache = SettlementCache::new();
        let reads = source.reads.get();

        cache.prepare(Some(&reader), &vp_at(CAM, 10.0), false);
        assert!(cache.is_empty() && cache.taken_for.is_none());
        assert_eq!(source.reads.get(), reads, "a disabled overlay runs no query");

        cache.prepare(Some(&reader), &vp_at(CAM, 1.0), true);
        assert!(cache.is_empty(), "no class shows a name at 1 m/px");
        assert_eq!(source.reads.get(), reads);

        // A filled cache empties when the overlay goes quiet, so nothing stale can be drawn.
        cache.prepare(Some(&reader), &vp_at(CAM, 10.0), true);
        assert!(!cache.is_empty());
        cache.prepare(Some(&reader), &vp_at(CAM, 10.0), false);
        assert!(cache.is_empty() && cache.taken_for.is_none());
    }

    #[test]
    fn a_full_cache_keeps_the_best_candidates() {
        // 40 villages, each one smaller than the last, all inside one view.
        let mut specs = StdVec::new();
        for i in 0..40i32 {
            specs.push(spec(
                47_500_000 + i * 200,
                7_500_000 + i * 200,
                SettlementClass::Village,
                &std::format!("V{i:02}"),
                (40 - i) as u16,
            ));
        }
        let bytes = build_poi_map(BBOX, 512, &[(obc_formats::obcm::SETTLEMENT_CATEGORY_ID, specs)]);
        reader_over!(bytes, source, reader);
        let mut cache = SettlementCache::new();
        cache.prepare(Some(&reader), &vp_at(CAM, 30.0), true);

        assert_eq!(cache.items.len(), MAX_CANDIDATES);
        let mut held: StdVec<_> = cache.items.iter().map(|c| c.population).collect();
        held.sort_unstable();
        let best: StdVec<u32> = (41 - MAX_CANDIDATES as u32..=40).map(|p| p * 100).collect();
        assert_eq!(held, best, "the largest populations are kept, whatever order they arrive in");
    }

    /// The padded region is taken from the four panel corners, so a turned camera is covered.
    #[test]
    fn the_padded_region_covers_a_rotated_view() {
        let vp = Viewport::new_rotated(
            PANEL.0,
            PANEL.1,
            CAM.0,
            CAM.1,
            obc_render::zoom_for_mpp(30.0),
            core::f32::consts::FRAC_PI_4,
        );
        let region = padded_bbox(&vp);
        let visible = vp.visible_bbox();
        assert!(region.contains(&visible), "the padded region holds the whole rotated view");
        assert!(region.min_lat < visible.min_lat && region.max_lat > visible.max_lat, "and it is larger");
        assert!(region.min_lon < visible.min_lon && region.max_lon > visible.max_lon);
    }

    // ---- the draw pass ----

    /// The class limits answer to the camera that draws. The browse screens fit a route into the
    /// panel at their own coarse scale, with a cache that was filled while riding.
    #[test]
    fn the_drawn_scale_decides_the_classes_whatever_the_cache_holds() {
        let riding = vp_at(CAM, 10.0);
        let cache = cache_of(&[
            at_screen(&riding, 120.0, 100.0, SettlementClass::City, "Freiburg", 230_000),
            at_screen(&riding, 120.0, 200.0, SettlementClass::Hamlet, "Hofsgrund", 300),
        ]);
        assert_eq!(drawn(&riding, &cache, &mut open_panel()), ["Freiburg", "Hofsgrund"], "both at 10 m/px");

        let fitted = vp_at(CAM, 100.0);
        assert_eq!(drawn(&fitted, &cache, &mut open_panel()), ["Freiburg"], "the hamlet is past its limit at 100 m/px");

        let far = vp_at(CAM, 900.0);
        assert!(drawn(&far, &cache, &mut open_panel()).is_empty(), "no class is named at 900 m/px");
    }

    #[test]
    fn a_label_that_crosses_the_panel_edge_is_dropped() {
        let vp = vp_at(CAM, 30.0);
        let cache = cache_of(&[
            at_screen(&vp, 6.0, 160.0, SettlementClass::City, "Westedge", 9),
            at_screen(&vp, 120.0, 318.0, SettlementClass::City, "Southedge", 8),
            at_screen(&vp, 120.0, 160.0, SettlementClass::City, "Middle", 7),
        ]);
        assert_eq!(drawn(&vp, &cache, &mut open_panel()), ["Middle"], "an edge label is dropped, never moved");
    }

    #[test]
    fn the_margin_suppresses_a_lower_priority_neighbour() {
        // 10 m/px, where every class shows a name.
        let vp = vp_at(CAM, 10.0);
        let close = cache_of(&[
            at_screen(&vp, 120.0, 150.0, SettlementClass::City, "Freiburg", 230_000),
            at_screen(&vp, 120.0, 158.0, SettlementClass::Hamlet, "Hofsgrund", 300),
        ]);
        assert_eq!(drawn(&vp, &close, &mut open_panel()), ["Freiburg"], "8 px apart is inside the 12 px margin");

        // The same pair with the panel rows the margin asks for: the label is 24 px tall, so the
        // second name clears at 36 px.
        let clear = cache_of(&[
            at_screen(&vp, 120.0, 150.0, SettlementClass::City, "Freiburg", 230_000),
            at_screen(&vp, 120.0, 186.0, SettlementClass::Hamlet, "Hofsgrund", 300),
        ]);
        assert_eq!(drawn(&vp, &clear, &mut open_panel()), ["Freiburg", "Hofsgrund"]);
    }

    #[test]
    fn the_reserved_chrome_and_the_rider_refuse_a_label() {
        let vp = vp_at(CAM, 30.0);
        let rider = rect(108, 148, 24, 24);
        let chip_band = rect(0, 250, 240, 70);
        let cache = cache_of(&[
            at_screen(&vp, 120.0, 150.0, SettlementClass::City, "Onrider", 9),
            at_screen(&vp, 120.0, 270.0, SettlementClass::City, "Onchip", 8),
            at_screen(&vp, 120.0, 60.0, SettlementClass::City, "Clear", 7),
        ]);
        let mut place = PointPlacement::new(rect(0, 0, 240, 320), &[rider, chip_band]);
        assert_eq!(drawn(&vp, &cache, &mut place), ["Clear"]);
    }

    #[test]
    fn a_long_name_is_cut_with_two_dots() {
        let vp = vp_at(CAM, 30.0);
        let cache = cache_of(&[at_screen(&vp, 120.0, 160.0, SettlementClass::Town, "Sankt Peter im Tal", 5_000)]);
        assert_eq!(drawn(&vp, &cache, &mut open_panel()), ["Sankt Pete.."]);
    }

    #[test]
    fn at_most_six_labels_are_drawn() {
        let vp = vp_at(CAM, 30.0);
        let mut items = StdVec::new();
        for i in 0..12 {
            items.push(at_screen(
                &vp,
                60.0 + (i % 2) as f32 * 120.0,
                60.0 + (i / 2) as f32 * 40.0,
                SettlementClass::Village,
                &std::format!("V{i:02}"),
                (12 - i) as u32 * 100,
            ));
        }
        let cache = cache_of(&items);
        assert_eq!(drawn(&vp, &cache, &mut open_panel()).len(), MAX_LABELS);
    }

    /// Stability: the same candidates under a moved camera keep their labels, and a better
    /// settlement that enters takes the space of the one it collides with.
    #[test]
    fn labels_keep_their_slots_across_a_pan_until_a_better_one_arrives() {
        let vp = vp_at(CAM, 30.0);
        let village = at_screen(&vp, 120.0, 160.0, SettlementClass::Village, "Denzlingen", 13_000);
        let far = at_screen(&vp, 90.0, 60.0, SettlementClass::Village, "Vörstetten", 3_000);
        let before = cache_of(&[village.clone(), far.clone()]);
        assert_eq!(drawn(&vp, &before, &mut open_panel()), ["Denzlingen", "Vörstetten"]);

        // A 20 px pan: the same set, a different camera. Both names stay.
        let (lon, lat) = vp.to_map(PANEL.0 / 2.0 + 20.0, PANEL.1 / 2.0);
        let panned = vp_at((lon, lat), 30.0);
        assert_eq!(drawn(&panned, &before, &mut open_panel()), ["Denzlingen", "Vörstetten"]);

        // A city enters, on the village's row. It outranks the village and takes the space.
        let city = at_screen(&panned, 124.0, 162.0, SettlementClass::City, "Freiburg", 230_000);
        let after = cache_of(&[village, far, city]);
        assert_eq!(drawn(&panned, &after, &mut open_panel()), ["Freiburg", "Vörstetten"]);
    }
}
