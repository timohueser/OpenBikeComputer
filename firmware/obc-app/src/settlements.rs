//! Settlement names on the map: a bounded candidate cache and the greedy label pass.
//!
//! The cache is refilled outside the draw path, over a viewport padded by [`CANDIDATE_PAD_PX`], and
//! only when the map, the scale band or the padded region stops covering the view. A frame then
//! orders the held candidates by [`rank`] and offers them to the shared
//! [`PointPlacement`](crate::screen::map::placement::PointPlacement), best first, until
//! [`MAX_LABELS`] are placed. [`rank`] holds no camera value, which is the whole stability
//! mechanism: priority decides every frame from scratch, with no history and no timer.

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
/// budget, so a name refused for space gives way to the next one rather than to nothing.
const MAX_CANDIDATES: usize = 16;
/// Labels drawn in one frame. One constant for every scale.
pub(crate) const MAX_LABELS: usize = 6;
/// The face the names are drawn in: one tier below the chrome a name sits among, because the name
/// annotates a place the rider already sees. Every metric below follows from this.
const LABEL_FONT: Font = Font::Caption;
/// Clear space around a label, in pixels: one glyph cell of [`LABEL_FONT`].
const LABEL_MARGIN_PX: i32 = 10;
/// Width shown, cut with `..`. A pinned name is never refused for width, so this decides only how
/// much of a long name reads. 140 pixels is 14 characters at the 10 by 20 face, so two names still
/// cannot sit side by side on the 240 px panel.
const MAX_LABEL_PX: i32 = 140;
/// Slack around the panel, so a small pan needs no new query.
const CANDIDATE_PAD_PX: f32 = 48.0;
/// The scale band, in metres per pixel, in which each class shows a name: the class, then `min`,
/// then `max`. The only place a scale decision is written. The 240 px panel is 9.6 km wide at
/// 40 m/px, about the size of a city, and 2.9 km at 12 m/px, about the size of a town. Under the
/// minimum the rider is inside the place and the name only covers the roads it is riding.
const CLASS_BANDS: [(SettlementClass, f32, f32); 4] = [
    (SettlementClass::City, 40.0, 600.0),
    (SettlementClass::Town, 12.0, 180.0),
    (SettlementClass::Village, 4.0, 50.0),
    (SettlementClass::Hamlet, 2.0, 16.0),
];

/// The classes that show a name at `mpp`, as a bit for each [`SettlementClass`] discriminant. Zero
/// means the overlay is quiet: nothing is queried and nothing is drawn.
fn class_mask(mpp: f32) -> u8 {
    if !(mpp.is_finite() && mpp > 0.0) {
        return 0;
    }
    let mut mask = 0;
    for (class, min, max) in CLASS_BANDS {
        if (min..=max).contains(&mpp) {
            mask |= 1 << class as u8;
        }
    }
    mask
}

/// One held settlement: the reader's record without its source identity, 44 bytes on the device.
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
/// then the position. It holds no camera value, so it does not change while the rider moves.
fn rank(c: &Candidate) -> (u8, Reverse<u32>, u8, i32, i32) {
    (c.class as u8, Reverse(c.population), c.name.chars().count() as u8, c.lat, c.lon)
}

/// What a held candidate set is for. A set taken for the same map and the same scale band is the
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

    /// Refill the candidates for `vp` when the held set no longer answers for it. A read failure is
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
/// corners go in, so a rotated camera is covered.
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

/// The name a label shows: [`MAX_LABEL_PX`] of [`LABEL_FONT`], cut with `..`.
fn label_of(c: &Candidate) -> Fitted {
    fit(c.name.as_str(), MAX_LABEL_PX, LABEL_FONT)
}

/// Draw the settlement names, best first, into whatever space `place` still has. Text is drawn
/// through the shared halo, and never rotated: a name reads upright at every heading.
pub(crate) fn draw_labels(cv: &mut impl Surface, vp: &Viewport, cache: &SettlementCache, place: &mut PointPlacement) {
    for (at, i) in placements(vp, cache, place) {
        halo_text(cv, &label_of(&cache.items[usize::from(i)]), at, LABEL_FONT, TextAlign::Center, INK, PARCHMENT);
    }
}

/// The labels this frame draws, as `(draw point, candidate index)` in the order they were placed.
///
/// A name is pinned to its place: the box is centred on it and never moves, so a name near an edge
/// hangs over it and the panel clips what falls outside. The overlap tests use the whole unclipped
/// box, so two names never overprint and a clipped name still holds the chrome off. A name is
/// dropped when its place is off the panel, or when `place` refuses the box. The set is re-ordered
/// by [`rank`] every frame, so a lower-priority name never holds a slot a better one needs, and a
/// class past its scale band for this camera is skipped whatever the cache holds.
fn placements(vp: &Viewport, cache: &SettlementCache, place: &mut PointPlacement) -> Vec<(Point, u8), MAX_LABELS> {
    // The scale limits answer to the camera that draws, which is not always the one the cache was
    // filled for: the browse screens fit a whole route into the panel at a much coarser scale.
    let mask = class_mask(vp.meters_per_pixel());
    let (w, h) = (vp.w as i32, vp.h as i32);
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
        if !(0..w).contains(&x) || !(0..h).contains(&y) {
            continue;
        }
        let tw = text_width(&label_of(c), LABEL_FONT) as i32;
        let th = LABEL_FONT.line_height() as i32;
        let top = y - th / 2;
        if place.try_place(rect(x - tw / 2, top, tw, th), LABEL_MARGIN_PX) {
            // `halo_text` centres on x and takes y as the top of the line.
            let _ = placed.push((Point::new(x, top), i));
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

    /// A placer with no reserved chrome.
    fn open_panel() -> PointPlacement {
        PointPlacement::new(&[])
    }

    /// The names a frame draws, in placement order.
    fn drawn(vp: &Viewport, cache: &SettlementCache, place: &mut PointPlacement) -> StdVec<String> {
        drawn_at(vp, cache, place).into_iter().map(|(name, _)| name).collect()
    }

    /// The names a frame draws with their draw points: the centre x and the top y of the text line.
    fn drawn_at(vp: &Viewport, cache: &SettlementCache, place: &mut PointPlacement) -> StdVec<(String, Point)> {
        placements(vp, cache, place)
            .into_iter()
            .map(|(at, i)| (label_of(&cache.items[usize::from(i)]).as_str().into(), at))
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
        assert_eq!(class_mask(30.0), 0b0110, "at 30 m/px a city no longer fits the panel; towns and villages do");
        assert_eq!(class_mask(10.0), 0b1100, "villages and hamlets at 10 m/px");
        assert_eq!(class_mask(3.0), 0b1000, "only hamlets at 3 m/px");
        assert_eq!(class_mask(1.0), 0, "inside a place the name only covers the roads");
        assert_eq!(class_mask(700.0), 0, "past the city limit nothing is named");

        // Both edges of every band.
        for (class, min, max) in CLASS_BANDS {
            let bit = 1 << class as u8;
            assert!(class_mask(min) & bit != 0, "{class:?} shows at {min} m/px");
            assert!(class_mask(min * 0.99) & bit == 0, "{class:?} is gone just under {min} m/px");
            assert!(class_mask(max) & bit != 0, "{class:?} shows at {max} m/px");
            assert!(class_mask(max * 1.01) & bit == 0, "{class:?} is gone just past {max} m/px");
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

    /// An unknown population ranks with a population of zero: a named place the map cannot size
    /// loses to one it can.
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
        let vp = vp_at(CAM, 30.0);
        let mut cache = SettlementCache::new();
        cache.prepare(Some(&reader), &vp, true);
        assert_eq!(cache.items.len(), 2, "the town and the village are in the band at 30 m/px");
        let region = cache.region;
        let reads = source.reads.get();

        let (lon, lat) = vp.to_map(PANEL.0 / 2.0 + 10.0, PANEL.1 / 2.0);
        cache.prepare(Some(&reader), &vp_at((lon, lat), 30.0), true);
        assert_eq!(source.reads.get(), reads, "a 10 px pan stays inside the padded region and reads nothing");
        assert_eq!(cache.region, region, "the held region is untouched");

        let (lon, lat) = vp.to_map(PANEL.0 / 2.0 + 200.0, PANEL.1 / 2.0);
        cache.prepare(Some(&reader), &vp_at((lon, lat), 30.0), true);
        assert!(source.reads.get() > reads, "a 200 px pan leaves the padded region and refills");
        assert_ne!(cache.region, region);
    }

    #[test]
    fn a_scale_change_that_crosses_a_class_limit_refills_the_cache() {
        let bytes = freiburg_map();
        reader_over!(bytes, source, reader);
        let mut cache = SettlementCache::new();
        cache.prepare(Some(&reader), &vp_at(CAM, 10.0), true);
        assert_eq!(cache.items.len(), 2, "the village and the hamlet at 10 m/px");
        let reads = source.reads.get();

        // 9 m/px holds the same two bands, over a narrower view the padded region still covers.
        cache.prepare(Some(&reader), &vp_at(CAM, 9.0), true);
        assert_eq!(source.reads.get(), reads, "a scale change that crosses no band edge reuses the set");

        cache.prepare(Some(&reader), &vp_at(CAM, 20.0), true);
        assert!(source.reads.get() > reads, "crossing the hamlet limit refills");
        let names: StdVec<_> = cache.items.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Emmendingen", "Denzlingen"],
            "the hamlet is out of its band at 20 m/px, and the town is in"
        );
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

    /// The class bands answer to the camera that draws. The browse screens fit a route into the
    /// panel at their own coarse scale, with a cache that was filled while riding.
    #[test]
    fn the_drawn_scale_decides_the_classes_whatever_the_cache_holds() {
        let riding = vp_at(CAM, 10.0);
        let cache = cache_of(&[
            at_screen(&riding, 120.0, 100.0, SettlementClass::City, "Freiburg", 230_000),
            at_screen(&riding, 120.0, 200.0, SettlementClass::Hamlet, "Hofsgrund", 300),
        ]);
        assert_eq!(drawn(&riding, &cache, &mut open_panel()), ["Hofsgrund"], "at 10 m/px a city does not fit");

        let fitted = vp_at(CAM, 100.0);
        assert_eq!(drawn(&fitted, &cache, &mut open_panel()), ["Freiburg"], "at 100 m/px the hamlet is a dot");

        let far = vp_at(CAM, 900.0);
        assert!(drawn(&far, &cache, &mut open_panel()).is_empty(), "no class is named at 900 m/px");
    }

    #[test]
    fn a_label_at_the_panel_edge_is_clipped_and_one_off_the_panel_is_dropped() {
        let vp = vp_at(CAM, 100.0);
        let cache = cache_of(&[
            at_screen(&vp, 6.0, 160.0, SettlementClass::City, "Westedge", 9),
            at_screen(&vp, -40.0, 100.0, SettlementClass::City, "Outside", 8),
        ]);
        let drawn = drawn_at(&vp, &cache, &mut open_panel());
        let names: StdVec<_> = drawn.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Westedge"], "a place off the panel has no label");
        let tw = text_width("Westedge", LABEL_FONT) as i32;
        assert_eq!(drawn[0].1.x, 6, "the name stays centred on its place");
        assert!(6 - tw / 2 < 0, "and the name is wide enough that the panel really clips it");
    }

    /// The exact boundary, derived from the constants so it holds whatever face the labels use: two
    /// names on the same column centred `separation` px apart leave
    /// `separation - LABEL_FONT.line_height()` of clear space, so [`LABEL_MARGIN_PX`] alone decides.
    #[test]
    fn the_margin_suppresses_a_lower_priority_neighbour() {
        // 30 m/px, where the town band and the village band overlap.
        let vp = vp_at(CAM, 30.0);
        let pair = |separation: f32| {
            cache_of(&[
                at_screen(&vp, 120.0, 150.0, SettlementClass::Town, "Emmendingen", 28_000),
                at_screen(&vp, 120.0, 150.0 + separation, SettlementClass::Village, "Maleck", 400),
            ])
        };
        let limit = (LABEL_FONT.line_height() as i32 + LABEL_MARGIN_PX) as f32;

        assert_eq!(
            drawn(&vp, &pair(limit - 1.0), &mut open_panel()),
            ["Emmendingen"],
            "one pixel short of the margin, the lower-priority name is refused"
        );
        assert_eq!(
            drawn(&vp, &pair(limit), &mut open_panel()),
            ["Emmendingen", "Maleck"],
            "a line height plus the margin apart, both names are drawn"
        );
    }

    #[test]
    fn the_reserved_chrome_and_the_rider_refuse_a_label() {
        let vp = vp_at(CAM, 100.0);
        let rider = rect(108, 148, 24, 24);
        let chip_band = rect(0, 250, 240, 70);
        let cache = cache_of(&[
            at_screen(&vp, 120.0, 150.0, SettlementClass::City, "Onrider", 9),
            at_screen(&vp, 120.0, 270.0, SettlementClass::City, "Onchip", 8),
            at_screen(&vp, 120.0, 60.0, SettlementClass::City, "Clear", 7),
        ]);
        let mut place = PointPlacement::new(&[rider, chip_band]);
        assert_eq!(drawn(&vp, &cache, &mut place), ["Clear"]);
    }

    /// On the real `sim-freiburg` map at the camera the simulator opens with, the city projects
    /// into the bottom-left corner, beside the scale bar. The bar inks a box about 33 px tall above
    /// the status chip, and the corner is not the bar, so the name is drawn.
    #[test]
    fn a_city_beside_the_scale_bar_is_placed() {
        // `initial_camera` on the fixture: the bbox centre, and the zoom that fits its longitude
        // span across the 240 px panel.
        let vp = Viewport::new(PANEL.0, PANEL.1, 7_885_982, 48_053_537, PANEL.0 / 321_244.0);
        let cache = cache_of(&[candidate(SettlementClass::City, "Freiburg", 220_200, 47_996_090, 7_849_401)]);

        // The chrome this frame really owns: no fix, so the `No GPS Fix` chip is up and the scale
        // bar steps above its band.
        let h = PANEL.1 as i32;
        let w = PANEL.0 as i32;
        let bar = crate::screen::map::ScaleBar::new(
            h,
            crate::screen::map::CHIP_H,
            vp.meters_per_pixel(),
            crate::Units::Metric,
        )
        .expect("the fixture camera yields a bar")
        .ink();
        let pill = crate::screen::map::chip_band_box(w, h);
        let chrome = crate::screen::map::label_reserved(&vp, None, &[], &[pill, bar]);
        let mut place = PointPlacement::new(&chrome);

        // The city has to land beside that box, or the test pins nothing: a repack of the fixture
        // that moved it up the panel would leave a name with no chrome anywhere near it.
        let (x, y) = vp.to_screen(7_849_401, 47_996_090);
        let tw = text_width("Freiburg", Font::Label) as i32;
        let (left, bottom) = (x - tw / 2, y + Font::Label.line_height() as i32 / 2);
        assert!(left < bar.top_left.x + bar.size.width as i32, "the name overhangs the bar's columns");
        assert!((0..24).contains(&(bar.top_left.y - bottom)), "the name ends within 24 px above the bar");

        assert_eq!(drawn(&vp, &cache, &mut place), ["Freiburg"]);
    }

    #[test]
    fn a_long_name_is_cut_with_two_dots() {
        let vp = vp_at(CAM, 30.0);
        let cache = cache_of(&[at_screen(&vp, 120.0, 160.0, SettlementClass::Town, "Sankt Peter im Tal", 5_000)]);
        assert_eq!(drawn(&vp, &cache, &mut open_panel()), ["Sankt Peter.."]);
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

    /// The real places: east of Emmendingen at 30 m/px the town sits nearer the left edge than half
    /// its name is wide, so the pinned name hangs over the edge and the panel clips it. At this face
    /// its box is narrow enough to leave Maleck, a village of 400 mid-panel, its own name.
    #[test]
    fn a_wide_town_name_at_the_edge_is_drawn_and_pinned() {
        let vp = vp_at((7_881_900, 48_121_100), 30.0);
        let cache = cache_of(&[
            candidate(SettlementClass::Village, "Maleck", 400, 48_123_600, 7_889_400),
            candidate(SettlementClass::Town, "Emmendingen", 28_000, 48_121_100, 7_849_700),
        ]);
        let (x, _) = vp.to_screen(7_849_700, 48_121_100);
        let tw = text_width("Emmendingen", LABEL_FONT) as i32;
        assert!(x < tw / 2, "the place is nearer the left edge than half the name is wide");

        let drawn = drawn_at(&vp, &cache, &mut open_panel());
        let names: StdVec<_> = drawn.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Emmendingen", "Maleck"], "the town at the edge is drawn, best first");
        assert_eq!(drawn[0].1.x, x, "the name stays pinned to its place and the panel clips it");
    }

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

        // A town enters, on the village's row. It outranks the village and takes the space in the
        // same frame: the order is decided from scratch, and holding a slot buys nothing.
        let town = at_screen(&panned, 124.0, 162.0, SettlementClass::Town, "Emmendingen", 28_000);
        let after = cache_of(&[village, far, town]);
        assert_eq!(drawn(&panned, &after, &mut open_panel()), ["Emmendingen", "Vörstetten"]);
    }
}
