//! Bounded map marks. Storage is read only when a padded viewport or selection changes.
use embedded_graphics::prelude::Point;
use heapless::Vec;
use obc_formats::obcm::{poi_category_of, SUMMIT_SUBTYPE_ID};
use obc_map_scene::BBox;
use obc_reader::{
    landmarks::{map_section, LandmarkDirectory, LandmarkQuery, QueryProgress},
    reader::MapPointQuery,
    PoiCategory, PoiCategorySet, Reader,
};
use obc_render::{rect, Surface, Viewport};

use crate::Settings;

const CAPACITY: usize = 64;
const DRAW_LIMIT: usize = 24;
const GLYPH_SCALE: i32 = 2;
const HALO_RADIUS: i32 = 14;
// The shipped map schema adds non-index contours at LOD 10 (10 metres per pixel).
const FULL_CONTOURS_MAX_MPP: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    Peak,
    Landmark,
    Water,
    Campsite,
    Accommodation,
    Resupply,
    Pharmacy,
    BikeShop,
    Train,
}

impl Kind {
    fn service(cat: PoiCategory) -> Self {
        match cat {
            PoiCategory::Water => Self::Water,
            PoiCategory::Campsite => Self::Campsite,
            PoiCategory::Accommodation => Self::Accommodation,
            PoiCategory::Resupply => Self::Resupply,
            PoiCategory::Pharmacy => Self::Pharmacy,
            PoiCategory::BikeShop => Self::BikeShop,
            PoiCategory::Train => Self::Train,
        }
    }
    fn radius(self) -> i32 {
        if self == Self::Peak {
            8
        } else {
            HALO_RADIUS
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Mark {
    id: u64,
    position: (i32, i32),
    elevation: i16,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Selection {
    peaks: bool,
    landmarks: bool,
    categories: PoiCategorySet,
}
impl Selection {
    fn includes(self, kind: Kind) -> bool {
        match kind {
            Kind::Peak => self.peaks,
            Kind::Landmark => self.landmarks,
            _ => PoiCategory::ALL.into_iter().any(|cat| Kind::service(cat) == kind && self.categories.contains(cat)),
        }
    }
    fn for_view(settings: &Settings, mpp: f32) -> Self {
        let mut categories = PoiCategorySet::EMPTY;
        if settings.map_pois && mpp <= 10.0 {
            for (i, cat) in PoiCategory::ALL.into_iter().enumerate() {
                if settings.map_poi_categories & (1 << i) != 0 {
                    categories = categories.with(cat);
                }
            }
        }
        Self {
            peaks: settings.map_peaks && settings.map_contours && mpp <= FULL_CONTOURS_MAX_MPP,
            landmarks: settings.map_landmarks && mpp <= 20.0,
            categories,
        }
    }
}

struct LandmarkLoad {
    directory: LandmarkDirectory,
    query: LandmarkQuery,
    hits: Vec<obc_reader::landmarks::LandmarkHit, 8>,
}

pub(crate) struct MapIcons {
    marks: Vec<Mark, CAPACITY>,
    coverage: Option<BBox>,
    visible: Option<BBox>,
    selection: Option<Selection>,
    generation: u32,
    query: Option<MapPointQuery>,
    landmarks: Option<LandmarkLoad>,
    next_at: Option<u32>,
    retries: u8,
}
impl MapIcons {
    pub const fn new() -> Self {
        Self {
            marks: Vec::new(),
            coverage: None,
            visible: None,
            selection: None,
            generation: 0,
            query: None,
            landmarks: None,
            next_at: None,
            retries: 0,
        }
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    pub fn wake_in(&self, now: u32) -> Option<u32> {
        self.next_at.map(|due| if now.wrapping_sub(due) < 0x8000_0000 { 0 } else { due.wrapping_sub(now) })
    }

    pub fn prepare(&mut self, reader: Option<&Reader>, vp: &Viewport, settings: &Settings, now: u32) {
        let Some(reader) = reader else {
            *self = Self::new();
            return;
        };
        let selection = Selection::for_view(settings, vp.meters_per_pixel());
        let visible = vp.visible_bbox();
        let pending = self.query.is_some() || self.landmarks.is_some();
        let view_empty = !pending
            && self.visible != Some(visible)
            && !self.marks.iter().any(|mark| in_bounds(visible, mark.position));
        let fresh = self.generation != reader.generation()
            || self.selection != Some(selection)
            || view_empty
            || !self.coverage.is_some_and(|cover| {
                contains(cover, visible) && cover.max_lon - cover.min_lon <= 4 * (visible.max_lon - visible.min_lon)
            });
        if fresh {
            self.retries = 0;
        }
        let retry = !pending && self.wake_in(now) == Some(0);
        if fresh || retry {
            if self.generation != reader.generation() {
                self.marks.clear();
            }
            self.generation = reader.generation();
            self.selection = Some(selection);
            self.visible = Some(visible);
            let dx = (visible.max_lon - visible.min_lon) / 2;
            let dy = (visible.max_lat - visible.min_lat) / 2;
            let bounds = BBox {
                min_lon: visible.min_lon.saturating_sub(dx),
                max_lon: visible.max_lon.saturating_add(dx),
                min_lat: visible.min_lat.saturating_sub(dy),
                max_lat: visible.max_lat.saturating_add(dy),
            };
            // Reproject valid cached marks while the bounded query fills the new view.
            self.marks.retain(|mark| selection.includes(mark.kind) && in_bounds(bounds, mark.position));
            self.coverage = Some(bounds);
            self.query = Some(MapPointQuery::new(self.generation, bounds, selection.categories, selection.peaks));
            self.landmarks = None;
            if selection.landmarks {
                match self.start_landmarks(reader, bounds) {
                    Ok(load) => self.landmarks = load,
                    Err(_) => {
                        self.failed(now);
                        return;
                    }
                }
            }
        } else if self.wake_in(now) != Some(0) {
            return;
        }
        if self.advance(reader).is_err() {
            self.failed(now);
            return;
        }
        self.next_at = (self.query.is_some() || self.landmarks.is_some()).then_some(now.wrapping_add(1));
    }

    fn failed(&mut self, now: u32) {
        self.query = None;
        self.landmarks = None;
        // Two delayed retries recover transient reads; a persistent failure does not spin.
        self.next_at = (self.retries < 2).then_some(now.wrapping_add(1000));
        self.retries = self.retries.saturating_add(1);
    }

    fn start_landmarks(&self, reader: &Reader, bounds: BBox) -> Result<Option<LandmarkLoad>, obc_reader::Error> {
        let Some(source) = map_section(reader.source())? else {
            return Ok(None);
        };
        let directory = LandmarkDirectory::read(&source)?;
        let center = (bounds.min_lon / 2 + bounds.max_lon / 2, bounds.min_lat / 2 + bounds.max_lat / 2);
        let radius =
            obc_map_scene::ground_dist_m_cl(center, (bounds.max_lon, bounds.max_lat), obc_map_scene::cos_lat(center.1))
                as u32
                + 1;
        Ok(Some(LandmarkLoad {
            directory,
            query: LandmarkQuery::new(directory, self.generation, center, radius, None),
            hits: Vec::new(),
        }))
    }

    fn advance(&mut self, reader: &Reader) -> Result<(), obc_reader::Error> {
        if let Some(mut query) = self.query.take() {
            let mut done = false;
            for _ in 0..8 {
                done = query.step(reader, |point| {
                    let kind = if point.subtype == SUMMIT_SUBTYPE_ID {
                        Kind::Peak
                    } else if let Some(cat) = poi_category_of(point.subtype) {
                        Kind::service(cat)
                    } else {
                        return;
                    };
                    self.retain(Mark {
                        id: point.source.0,
                        position: point.position,
                        elevation: point.elevation_m.unwrap_or(i16::MIN),
                        kind,
                    });
                })?;
                if done {
                    break;
                }
            }
            if !done {
                self.query = Some(query);
            }
        }
        if let Some(mut load) = self.landmarks.take() {
            let source = map_section(reader.source())?.ok_or(obc_reader::Error::BadOffset)?;
            let mut done = false;
            for _ in 0..8 {
                match load.query.step(&source, load.directory, self.generation, &mut load.hits) {
                    QueryProgress::Pending => {}
                    QueryProgress::Ready { .. } => {
                        done = true;
                        break;
                    }
                    QueryProgress::Failed(error) => return Err(error),
                    QueryProgress::Cancelled => return Err(obc_reader::Error::BadOffset),
                }
            }
            // Each step produces at most one hit. Consume this batch so the shared cache,
            // rather than a separate landmark page limit, chooses what stays visible.
            for hit in &load.hits {
                let record = load.directory.record(&source, hit.index)?;
                self.retain(Mark {
                    id: record.osm.map_or(record.qid, |osm| osm.source.0),
                    position: hit.position,
                    elevation: 0,
                    kind: Kind::Landmark,
                });
            }
            load.hits.clear();
            if !done {
                self.landmarks = Some(load);
            }
        }
        Ok(())
    }

    fn retention_rank(&self, mark: Mark) -> (bool, u64, i32, u64) {
        let bounds = self.visible.unwrap_or(BBox { min_lon: 0, max_lon: 0, min_lat: 0, max_lat: 0 });
        let center = (bounds.min_lon / 2 + bounds.max_lon / 2, bounds.min_lat / 2 + bounds.max_lat / 2);
        let dx = i64::from(mark.position.0) - i64::from(center.0);
        let dy = i64::from(mark.position.1) - i64::from(center.1);
        (!in_bounds(bounds, mark.position), (dx * dx + dy * dy) as u64, -i32::from(mark.elevation), mark.id)
    }

    fn retain(&mut self, mark: Mark) {
        if self
            .marks
            .iter()
            .any(|old| (mark.id != 0 && old.id == mark.id) || (old.kind == mark.kind && old.position == mark.position))
        {
            return;
        }
        if !self.marks.is_full() {
            let _ = self.marks.push(mark);
            return;
        }
        let mut counts = [0u8; 9];
        for old in &self.marks {
            counts[old.kind as usize] += 1;
        }
        let largest = *counts.iter().max().unwrap();
        let own_count = counts[mark.kind as usize];
        let worst = self
            .marks
            .iter()
            .enumerate()
            .filter(
                |(_, old)| {
                    if own_count < largest {
                        counts[old.kind as usize] == largest
                    } else {
                        old.kind == mark.kind
                    }
                },
            )
            .max_by_key(|(_, old)| self.retention_rank(**old))
            .map(|(i, _)| i);
        if let Some(i) = worst {
            if own_count < largest || self.retention_rank(mark) < self.retention_rank(self.marks[i]) {
                self.marks[i] = mark;
            }
        }
    }

    pub fn draw(
        &self,
        cv: &mut impl Surface,
        vp: &Viewport,
        rider: Option<(i32, i32)>,
        waypoints: &[obc_route::WptEntry],
    ) {
        for (point, kind) in self.placements(vp, rider, waypoints) {
            draw_glyph(cv, point, kind);
        }
    }

    fn placements(
        &self,
        vp: &Viewport,
        rider: Option<(i32, i32)>,
        waypoints: &[obc_route::WptEntry],
    ) -> Vec<(Point, Kind), DRAW_LIMIT> {
        let mpp = vp.meters_per_pixel();
        let limit = if mpp > 20.0 {
            8
        } else if mpp > 5.0 {
            16
        } else {
            DRAW_LIMIT
        };
        // Sort only small indices, leaving the persistent geographic cache untouched.
        let mut order: Vec<u8, CAPACITY> = (0..self.marks.len() as u8).collect();
        order.sort_unstable_by_key(|&i| {
            let mark = self.marks[i as usize];
            let (x, y) = vp.to_screen(mark.position.0, mark.position.1);
            let dx = i64::from(x) - vp.w as i64 / 2;
            let dy = i64::from(y) - vp.h as i64 / 2;
            (dx * dx + dy * dy, mark.id, mark.kind as u8, mark.position)
        });
        let mut placed = Vec::<(Point, Kind), DRAW_LIMIT>::new();
        let mut counts = [0u8; 9];
        let mut rejected = 0u64;
        let rider = rider.map(|(lon, lat)| vp.to_screen(lon, lat));
        for round in 0..limit as u8 {
            let before = placed.len();
            for &i in &order {
                if placed.len() == limit {
                    return placed;
                }
                let mark = self.marks[i as usize];
                if rejected & (1u64 << i) != 0 || counts[mark.kind as usize] != round {
                    continue;
                }
                rejected |= 1u64 << i;
                let (x, y) = vp.to_screen(mark.position.0, mark.position.1);
                let radius = mark.kind.radius();
                // Keep the clock, battery, scale, warning chip and pan controls free.
                if x < radius + 2 || x > vp.w as i32 - radius - 2 || y < radius + 31 || y > vp.h as i32 - radius - 51 {
                    continue;
                }
                if rider.is_some_and(|(rx, ry)| near(x, y, rx, ry, radius + 17)) {
                    continue;
                }
                if waypoints.iter().any(|wp| {
                    let (wx, wy) = vp.to_screen(wp.lon, wp.lat);
                    near(x, y, wx, wy, radius + 9)
                }) {
                    continue;
                }
                if placed
                    .iter()
                    .any(|(p, kind)| near(x, y, p.x, p.y, radius + kind.radius() + if mpp > 20.0 { 12 } else { 2 }))
                {
                    continue;
                }
                let _ = placed.push((Point::new(x, y), mark.kind));
                counts[mark.kind as usize] += 1;
            }
            if placed.len() == before {
                break;
            }
        }
        placed
    }
}

fn in_bounds(b: BBox, p: (i32, i32)) -> bool {
    p.0 >= b.min_lon && p.0 <= b.max_lon && p.1 >= b.min_lat && p.1 <= b.max_lat
}
fn contains(a: BBox, b: BBox) -> bool {
    a.min_lon <= b.min_lon && a.max_lon >= b.max_lon && a.min_lat <= b.min_lat && a.max_lat >= b.max_lat
}
fn near(x: i32, y: i32, a: i32, b: i32, r: i32) -> bool {
    (x - a).abs() < r && (y - b).abs() < r
}

// Scale the compact bitmaps to legible device pixels without a larger asset or cache.
fn draw_glyph(cv: &mut impl Surface, p: Point, kind: Kind) {
    let rows: [u16; 11] = match kind {
        Kind::Peak => {
            cv.triangle(p + Point::new(0, -4), p + Point::new(-7, 3), p + Point::new(7, 3), 0x0000);
            return;
        }
        Kind::Landmark => [
            0,
            0b00000100000,
            0b00011111000,
            0b01111111110,
            0b11111111111,
            0,
            0b01001001010,
            0b01001001010,
            0b01001001010,
            0b11111111111,
            0,
        ],
        Kind::Water => [
            0b00000100000,
            0b00001110000,
            0b00001110000,
            0b00011111000,
            0b00111111100,
            0b01111111110,
            0b01101111110,
            0b01111111110,
            0b00111111100,
            0b00011111000,
            0,
        ],
        Kind::Campsite => [
            0,
            0b00000100000,
            0b00001110000,
            0b00011111000,
            0b00111011100,
            0b00111011100,
            0b01110001110,
            0b11110001111,
            0b11100000111,
            0b11111111111,
            0,
        ],
        Kind::Accommodation => [
            0,
            0,
            0b10000000000,
            0b10110000000,
            0b10110000000,
            0b11111111110,
            0b11111111111,
            0b10000000001,
            0b10000000001,
            0,
            0,
        ],
        Kind::Resupply => [
            0b00011111000,
            0b00100000100,
            0b00100000100,
            0b11111111111,
            0b01000000010,
            0b01010101010,
            0b00101010100,
            0b00100000100,
            0b00011111000,
            0,
            0,
        ],
        Kind::Pharmacy => [
            0,
            0b00011100000,
            0b00011100000,
            0b00011100000,
            0b11111111100,
            0b11111111100,
            0b11111111100,
            0b00011100000,
            0b00011100000,
            0b00011100000,
            0,
        ],
        Kind::BikeShop => [
            0,
            0b00110001100,
            0b00010010000,
            0b00010110000,
            0b01111111110,
            0b10011100101,
            0b10101010101,
            0b10001010001,
            0b01110001110,
            0,
            0,
        ],
        Kind::Train => [
            0b00111111100,
            0b01000000010,
            0b01011111010,
            0b01011111010,
            0b01000000010,
            0b01010001010,
            0b01111111110,
            0b00100000100,
            0b01000000010,
            0b01111111110,
            0,
        ],
    };
    cv.disc(p, HALO_RADIUS as u32, 0x8410);
    cv.disc(p, (HALO_RADIUS - 1) as u32, 0xffff);
    let ink = if kind == Kind::Water { 0x0015 } else { 0x2104 };
    for (y, bits) in rows.into_iter().enumerate() {
        for x in 0..11 {
            if bits & (1 << (10 - x)) != 0 {
                cv.fill(
                    rect(p.x + x * GLYPH_SCALE - 11, p.y + y as i32 * GLYPH_SCALE - 11, GLYPH_SCALE, GLYPH_SCALE),
                    ink,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use obc_formats::io::{ByteSource, Error, SliceSource};
    use obc_reader::{MapCache, MapTables};
    use obcm_testkit::{build_poi_map, PoiSpec};

    struct CountSource<'a> {
        bytes: &'a [u8],
        reads: Cell<usize>,
        fail: Cell<bool>,
    }
    impl ByteSource for CountSource<'_> {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<(), Error> {
            self.reads.set(self.reads.get() + 1);
            if self.fail.get() {
                return Err(Error::Io);
            }
            SliceSource(self.bytes).read_at(off, dst)
        }
    }
    fn map() -> std::vec::Vec<u8> {
        let mut cats = std::vec::Vec::new();
        for (i, cat) in PoiCategory::ALL.into_iter().enumerate() {
            let subtype = [1, 5, 7, 13, 17, 18, 20][i];
            cats.push((
                cat.id(),
                std::vec![PoiSpec {
                    lat: 46_000_000,
                    lon: 8_000_000 + 100 * i as i32,
                    subtype,
                    name: "".into(),
                    hours_ref: 0xffff
                }],
            ));
        }
        cats.push((
            7,
            std::vec![PoiSpec { lat: 46_000_500, lon: 8_000_000, subtype: 19, name: "".into(), hours_ref: 3000 }],
        ));
        build_poi_map((7_900_000, 45_900_000, 8_100_000, 46_100_000), 512, &cats)
    }
    fn settle(icons: &mut MapIcons, reader: Option<&Reader>, vp: &Viewport, settings: &Settings) {
        let mut now = 0;
        for _ in 0..2000 {
            icons.prepare(reader, vp, settings, now);
            let Some(delay) = icons.wake_in(now) else {
                return;
            };
            now += delay.max(1);
        }
        panic!("query did not settle");
    }
    fn view() -> Viewport {
        Viewport::new(240.0, 320.0, 8_000_000, 46_000_000, obc_render::zoom_for_mpp(3.0))
    }

    #[test]
    fn selected_categories_unnamed_peaks_cache_rotation_replacement_and_read_failure() {
        let bytes = map();
        let source = CountSource { bytes: &bytes, reads: Cell::new(0), fail: Cell::new(false) };
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut icons = MapIcons::new();
        let mut settings = Settings { map_landmarks: false, ..Settings::default() };
        let vp = view();
        settle(&mut icons, Some(&reader), &vp, &settings);
        assert_eq!(icons.marks.len(), 8, "all seven categories and unnamed summit");
        assert!(icons.marks.iter().any(|m| m.kind == Kind::Peak && m.elevation == 3000));
        let reads = source.reads.get();
        settle(&mut icons, Some(&reader), &vp, &settings);
        let rotated = Viewport::new_rotated(240.0, 320.0, vp.cam_lon + 50, vp.cam_lat, vp.zoom, 0.3);
        settle(&mut icons, Some(&reader), &rotated, &settings);
        assert_eq!(reads, source.reads.get(), "pan and rotation inside cover do not read storage");
        let zoomed = Viewport::new(240.0, 320.0, vp.cam_lon, vp.cam_lat, obc_render::zoom_for_mpp(9.0));
        icons.prepare(Some(&reader), &zoomed, &settings, 1000);
        assert_eq!(icons.marks.len(), 8, "zoom retains existing marks before refill finishes");
        assert!(icons.query.is_some(), "transition is exercised during a partial query");
        assert_eq!(icons.wake_in(1000), Some(1), "refill resumes on the next available frame");
        settings.map_poi_categories = 3;
        icons.prepare(Some(&reader), &vp, &settings, 1001);
        assert_eq!(icons.marks.len(), 3, "disabled categories disappear without blanking enabled marks");
        settle(&mut icons, Some(&reader), &vp, &settings);
        assert_eq!(icons.marks.len(), 3);
        settings.map_pois = false;
        settings.map_peaks = false;
        let reads = source.reads.get();
        settle(&mut icons, Some(&reader), &vp, &settings);
        assert!(icons.marks.is_empty());
        assert_eq!(reads, source.reads.get(), "hidden categories cause no storage queries");
        settings.map_pois = true;
        settle(&mut icons, Some(&reader), &vp, &settings);
        assert_eq!(icons.marks.len(), 2, "master switch preserves water/campsite selection");
        source.fail.set(true);
        let replacement = MapTables::parse(&SliceSource(&bytes)).unwrap();
        let reader = Reader::new(&source, &replacement, &cache);
        settle(&mut icons, Some(&reader), &vp, &settings);
        assert!(icons.marks.is_empty(), "replacement never retains stale marks after failed reads");
        settle(&mut icons, None, &vp, &settings);
        assert!(icons.coverage.is_none());
    }

    #[test]
    fn crowding_priorities_protect_rider_and_chrome_with_bounded_stable_output() {
        let vp = view();
        let mut icons = MapIcons::new();
        for i in 0..100 {
            let position = vp.to_map(20.0 + (i % 10) as f32 * 21.0, 45.0 + (i / 10) as f32 * 22.0);
            for kind in [
                Kind::Peak,
                Kind::Landmark,
                Kind::Water,
                Kind::Campsite,
                Kind::Accommodation,
                Kind::Resupply,
                Kind::Pharmacy,
                Kind::BikeShop,
                Kind::Train,
            ] {
                icons.retain(Mark { id: i + 1 + (kind as u64) * 100, position, elevation: 1000, kind });
            }
        }
        assert_eq!(icons.marks.len(), CAPACITY);
        let rider = (vp.cam_lon, vp.cam_lat);
        let placed = icons.placements(&vp, Some(rider), &[]);
        assert!(!placed.is_empty() && placed.len() <= DRAW_LIMIT);
        assert_eq!(placed, icons.placements(&vp, Some(rider), &[]));
        for (i, (p, kind)) in placed.iter().enumerate() {
            let radius = kind.radius();
            assert!(p.y >= radius + 31 && p.y <= 320 - radius - 51);
            assert!(!near(p.x, p.y, 120, 160, radius + 17));
            assert!(!placed[..i].iter().any(|(q, other)| near(p.x, p.y, q.x, q.y, radius + other.radius() + 2)));
        }
        let mut settings = Settings::default();
        assert!(Selection::for_view(&settings, 10.01).categories.is_empty());
        assert!(Selection::for_view(&settings, 10.0).peaks);
        assert!(!Selection::for_view(&settings, 10.01).peaks, "index-only contours do not show peaks");
        settings.map_contours = false;
        assert!(!Selection::for_view(&settings, 1.0).peaks, "the contour switch also hides peaks");
        settings.map_contours = true;
        settings.map_peaks = false;
        assert!(!Selection::for_view(&settings, 1.0).peaks);
    }
    #[test]
    fn crowded_views_offer_nearest_from_each_category_before_seconds() {
        let vp = view();
        let kinds = [
            Kind::Peak,
            Kind::Landmark,
            Kind::Water,
            Kind::Campsite,
            Kind::Accommodation,
            Kind::Resupply,
            Kind::Pharmacy,
            Kind::BikeShop,
            Kind::Train,
        ];
        let mut icons = MapIcons::new();
        for i in 0..27 {
            icons.retain(Mark {
                id: i as u64 + 1,
                position: vp.to_map(20.0 + (i % 5) as f32 * 45.0, 47.0 + (i / 5) as f32 * 40.0),
                elevation: 1000,
                kind: kinds[i % kinds.len()],
            });
        }
        let placed = icons.placements(&vp, None, &[]);
        assert_eq!(placed.len(), DRAW_LIMIT);
        for round in placed[..18].as_chunks::<9>().0 {
            for kind in kinds {
                assert_eq!(round.iter().filter(|(_, k)| *k == kind).count(), 1);
            }
        }
        for kind in kinds {
            let nearest = icons
                .marks
                .iter()
                .filter(|mark| mark.kind == kind)
                .map(|mark| {
                    let (x, y) = vp.to_screen(mark.position.0, mark.position.1);
                    ((x - 120).pow(2) + (y - 160).pow(2), Point::new(x, y))
                })
                .min_by_key(|(distance, _)| *distance)
                .unwrap()
                .1;
            assert_eq!(placed.iter().find(|(_, k)| *k == kind).unwrap().0, nearest);
        }
        icons.marks.reverse();
        assert_eq!(placed, icons.placements(&vp, None, &[]), "source order does not change selection");
        for mark in &mut icons.marks {
            mark.kind = Kind::Water;
        }
        assert_eq!(icons.placements(&vp, None, &[]).len(), DRAW_LIMIT, "one category may fill all free slots");
    }

    #[test]
    fn full_cache_reserves_space_for_sparse_categories() {
        let mut icons = MapIcons::new();
        for i in 0..CAPACITY {
            icons.retain(Mark { id: i as u64 + 1, position: (i as i32, 0), elevation: 0, kind: Kind::Water });
        }
        icons.retain(Mark { id: 100, position: (1000, 0), elevation: 0, kind: Kind::Campsite });
        assert_eq!(icons.marks.len(), CAPACITY);
        assert!(icons.marks.iter().any(|m| m.kind == Kind::Campsite));
        assert!(!icons.marks.iter().any(|m| m.id == 64), "farthest item from the crowded category yields its slot");
    }

    #[test]
    fn visible_candidates_beat_padding_and_empty_pans_refill_with_bounded_reads() {
        let vp = view();
        let mut pois = std::vec::Vec::new();
        for i in 0..68 {
            let position = vp.to_map(if i < 64 { 60.0 + i as f32 * 0.1 } else { 300.0 + (i - 64) as f32 }, 160.0);
            pois.push(PoiSpec { lon: position.0, lat: position.1, subtype: 1, name: "".into(), hours_ref: 0xffff });
        }
        let bytes = build_poi_map((7_900_000, 45_900_000, 8_100_000, 46_100_000), 512, &[(1, pois)]);
        let source = CountSource { bytes: &bytes, reads: Cell::new(0), fail: Cell::new(false) };
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let settings =
            Settings { map_peaks: false, map_landmarks: false, map_poi_categories: 1, ..Settings::default() };
        let mut icons = MapIcons::new();
        settle(&mut icons, Some(&reader), &vp, &settings);
        assert_eq!(icons.marks.len(), CAPACITY);
        assert!(icons.marks.iter().all(|m| in_bounds(vp.visible_bbox(), m.position)));
        let (lon, lat) = vp.to_map(239.0, 160.0);
        let panned = Viewport::new(240.0, 320.0, lon, lat, vp.zoom);
        assert!(contains(icons.coverage.unwrap(), panned.visible_bbox()));
        assert!(!icons.marks.iter().any(|m| in_bounds(panned.visible_bbox(), m.position)));
        settle(&mut icons, Some(&reader), &panned, &settings);
        assert!(icons.marks.iter().any(|m| in_bounds(panned.visible_bbox(), m.position)));
        let mut query = MapPointQuery::new(reader.generation(), vp.visible_bbox(), PoiCategorySet::ALL, true);
        for _ in 0..1000 {
            let before = source.reads.get();
            let done = query.step(&reader, |_| {}).unwrap();
            assert!(source.reads.get() - before <= 1, "one query step performs at most one source read");
            if done {
                return;
            }
        }
        panic!("bounded walk did not finish");
    }

    #[test]
    fn transient_reads_retry_and_completed_or_persistent_failure_stops_waking() {
        let bytes = map();
        let source = CountSource { bytes: &bytes, reads: Cell::new(0), fail: Cell::new(false) };
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let settings = Settings { map_landmarks: false, ..Settings::default() };
        let mut icons = MapIcons::new();
        source.fail.set(true);
        icons.prepare(Some(&reader), &view(), &settings, 0);
        assert_eq!(icons.wake_in(0), Some(1000));
        let reads = source.reads.get();
        icons.prepare(Some(&reader), &view(), &settings, 999);
        assert_eq!(source.reads.get(), reads);
        source.fail.set(false);
        let mut now = 1000;
        for _ in 0..100 {
            icons.prepare(Some(&reader), &view(), &settings, now);
            if icons.wake_in(now).is_none() {
                break;
            }
            now += 50;
        }
        assert_eq!(icons.marks.len(), 8);
        assert!(icons.wake_in(now).is_none());
        let reads = source.reads.get();
        icons.prepare(Some(&reader), &view(), &settings, now + 5000);
        assert_eq!(source.reads.get(), reads);
        source.fail.set(true);
        icons = MapIcons::new();
        settle(&mut icons, Some(&reader), &view(), &settings);
        assert!(icons.marks.is_empty() && icons.wake_in(5000).is_none());
    }

    #[test]
    fn installed_landmarks_load_in_bounded_steps_and_visibility_is_independent() {
        use obc_formats::obcm::{self, landmarks::*};
        let mut bytes = map();
        let count = 100;
        let payload = SECTION_HEADER_LEN + count * RECORD_LEN;
        let mut section = std::vec![0;payload];
        section[..4].copy_from_slice(&(count as u32).to_le_bytes());
        section[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
        section[6..8].copy_from_slice(&SECTION_VERSION.to_le_bytes());
        section[8..12].copy_from_slice(&(payload as u32).to_le_bytes());
        section[12..16].copy_from_slice(&((payload + 4) as u32).to_le_bytes());
        for i in 0..count {
            let record = LandmarkRecord {
                qid: 1000 + i as u64,
                lon: 8_000_000 + i as i32 * 10,
                lat: 46_000_100,
                category: 2,
                hours_ref: 0xffff,
                osm: None,
                name: ContentRef { offset: payload as u32, len: 4 },
                articles: ContentRef::default(),
                photo: ContentRef::default(),
                photo_attribution: ContentRef::default(),
            };
            let start = SECTION_HEADER_LEN + i * RECORD_LEN;
            section[start..start + RECORD_LEN].copy_from_slice(&record.encode());
        }
        section.extend_from_slice(b"Site");
        let start = obcm_testkit::align_up(bytes.len());
        bytes.resize(start, 0);
        bytes.extend(section);
        bytes.resize(obcm_testkit::align_up(bytes.len()), 0);
        let len = bytes.len() - start;
        bytes[obcm::HEADER_LANDMARK_OFFSET_OFF..obcm::HEADER_LANDMARK_OFFSET_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(start).to_le_bytes());
        bytes[obcm::HEADER_LANDMARK_LENGTH_OFF..obcm::HEADER_LANDMARK_LENGTH_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(len).to_le_bytes());
        let source = CountSource { bytes: &bytes, reads: Cell::new(0), fail: Cell::new(false) };
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut icons = MapIcons::new();
        let settings = Settings { map_pois: false, map_peaks: false, ..Settings::default() };
        let mut frames = 0;
        for now in (0..5000).step_by(50) {
            let before = source.reads.get();
            icons.prepare(Some(&reader), &view(), &settings, now);
            assert!(source.reads.get() - before <= 20, "landmark scan never drains a latitude band in one frame");
            frames += 1;
            if icons.wake_in(now).is_none() {
                break;
            }
        }
        assert!(frames > 1);
        assert_eq!(icons.marks.len(), CAPACITY, "landmarks use the shared budget, not an eight-item page limit");
        assert!(icons.marks.iter().all(|mark| mark.kind == Kind::Landmark));
        let off = Settings { map_landmarks: false, ..settings };
        icons.prepare(Some(&reader), &view(), &off, 6000);
        assert!(icons.marks.is_empty(), "hidden marks disappear before refill completes");
    }
}
