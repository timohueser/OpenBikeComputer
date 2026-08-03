//! Target-independent core for the skin editor's production-rendered scene.

use embedded_graphics::pixelcolor::Rgb888;
use obc_formats::io::SliceSource;
use obc_formats::obcm::{HEADER_LEN, STYLE_RECORD_LEN};
use obc_host_core::RgbaFrame;
use obc_map_scene::BBox;
use obc_reader::{rgb565_to_device64, Error as ReadError, MapCache, MapTables, Reader};
use obc_render::{zoom_for_mpp, MapRenderer, RenderStats, Viewport};
use obcm_assemble::schema::{Schema, Skin};
use obcm_assemble::shard::pack_style_table;

pub const FRAME_W: u32 = 240;
pub const FRAME_H: u32 = 240;
pub const SCHEMA_FRAME_W: u32 = 240;
pub const SCHEMA_FRAME_H: u32 = 320;

const CAMERA_LON: i32 = 7_814_000;
const CAMERA_LAT: i32 = 48_130_000;
// The packer's requested crop. Complete OSM ways may legally extend the OBCM
// header beyond it; those overhangs are not evidence of dense preview coverage
// and must never become pannable just because their coordinates are present.
const TENINGEN_COVERAGE: BBox =
    BBox { min_lon: 7_798_000, min_lat: 48_119_000, max_lon: 7_830_000, max_lat: 48_141_000 };
const DEFAULT_METERS_PER_PIXEL: f32 = 5.0;
const MIN_METERS_PER_PIXEL: f32 = 0.5;
const MAX_MPP_SEARCH: f32 = 100_000.0;
const MAX_SCHEMA_MPP: f32 = 100_000.0;

const HEADER_STYLE_OFFSET_AT: usize = 21;
const HEADER_MARKER_COLOR_AT: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewErrorCode {
    NotAMap,
    Input,
    Internal,
}

impl PreviewErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewErrorCode::NotAMap => "not-a-map",
            PreviewErrorCode::Input => "input",
            PreviewErrorCode::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFailure {
    pub code: PreviewErrorCode,
    pub message: String,
}

/// Camera and production-renderer diagnostics for the last requested frame.
///
/// Kept in the Rust bridge so every browser surface reports the exact LOD and
/// budget accounting chosen by `obc-render`, not a TypeScript approximation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewStats {
    pub camera_lon: i32,
    pub camera_lat: i32,
    pub meters_per_pixel: f32,
    pub lod_index: usize,
    pub lod_count: usize,
    pub features_drawn: usize,
    pub features_dropped: usize,
    pub points_drawn: usize,
    pub span_utilization: f32,
    pub point_utilization: f32,
    pub ring_utilization: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Camera {
    lon: i32,
    lat: i32,
    meters_per_pixel: f32,
}

impl PreviewFailure {
    fn input(message: impl Into<String>) -> Self {
        Self { code: PreviewErrorCode::Input, message: message.into() }
    }
}

fn read_failure(err: ReadError) -> PreviewFailure {
    match err {
        ReadError::BadMagic | ReadError::BadVersion | ReadError::TooShort | ReadError::BadOffset => PreviewFailure {
            code: PreviewErrorCode::NotAMap,
            message: "The Teningen preview map is missing, stale, or truncated.".into(),
        },
        ReadError::Source(_) | ReadError::CacheBusy => PreviewFailure {
            code: PreviewErrorCode::Internal,
            message: "The Teningen preview map could not be read.".into(),
        },
    }
}

pub struct MapPreview {
    bytes: Vec<u8>,
    tables: MapTables,
    schema: Schema,
    cache: Box<MapCache>,
    renderer: Box<MapRenderer>,
    frame: RgbaFrame,
    camera: Camera,
    default_camera: Camera,
    min_mpp: f32,
    max_mpp: f32,
    camera_bounds: BBox,
    render_stats: RenderStats,
    dirty: bool,
}

/// A freshly native-packed schema preview. Unlike [`MapPreview`], these bytes
/// already carry the edited style table, so opening never accepts a skin or
/// rewrites the OBCM. The frame matches the device's complete 240×320 map plane.
pub struct SchemaMapPreview {
    bytes: Vec<u8>,
    tables: MapTables,
    cache: Box<MapCache>,
    renderer: Box<MapRenderer>,
    frame: RgbaFrame,
    meters_per_pixel: f32,
    render_stats: RenderStats,
    dirty: bool,
}

impl MapPreview {
    pub fn open(bytes: Vec<u8>, schema_json: &str, skin_json: &str) -> Result<Self, PreviewFailure> {
        let tables = MapTables::parse(&SliceSource(&bytes)).map_err(read_failure)?;
        let schema = Schema::parse(schema_json).map_err(PreviewFailure::input)?;
        schema.validate().map_err(PreviewFailure::input)?;
        let bbox = tables.bbox;
        let camera_bounds = if contains_bbox(bbox, TENINGEN_COVERAGE) { TENINGEN_COVERAGE } else { bbox };
        let center = (
            (camera_bounds.min_lon as i64 + camera_bounds.max_lon as i64) / 2,
            (camera_bounds.min_lat as i64 + camera_bounds.max_lat as i64) / 2,
        );
        let preferred = if camera_bounds.min_lon <= CAMERA_LON
            && CAMERA_LON <= camera_bounds.max_lon
            && camera_bounds.min_lat <= CAMERA_LAT
            && CAMERA_LAT <= camera_bounds.max_lat
        {
            (CAMERA_LON, CAMERA_LAT)
        } else {
            (center.0 as i32, center.1 as i32)
        };
        let max_mpp = maximum_fitting_mpp(bbox, camera_bounds);
        let min_mpp = MIN_METERS_PER_PIXEL.min(max_mpp);
        let camera = Camera {
            lon: preferred.0,
            lat: preferred.1,
            meters_per_pixel: DEFAULT_METERS_PER_PIXEL.clamp(min_mpp, max_mpp),
        };
        let mut preview = Self {
            bytes,
            tables,
            schema,
            cache: MapCache::new_boxed(),
            renderer: Box::new(MapRenderer::new()),
            frame: RgbaFrame::new(FRAME_W, FRAME_H),
            camera,
            default_camera: camera,
            min_mpp,
            max_mpp,
            camera_bounds,
            render_stats: RenderStats::default(),
            dirty: true,
        };
        preview.clamp_camera();
        preview.default_camera = preview.camera;
        preview.set_skin(skin_json)?;
        Ok(preview)
    }

    pub fn set_skin(&mut self, skin_json: &str) -> Result<(), PreviewFailure> {
        let skin = Skin::parse(skin_json).map_err(PreviewFailure::input)?;
        let styles = skin.resolve(&self.schema).map_err(PreviewFailure::input)?;

        if self.bytes.len() < HEADER_LEN {
            return Err(PreviewFailure::input("The Teningen preview is shorter than the OBCM header."));
        }
        let style_offset = u32::from_le_bytes(
            self.bytes[HEADER_STYLE_OFFSET_AT..HEADER_STYLE_OFFSET_AT + 4]
                .try_into()
                .expect("four bytes inside the checked header"),
        ) as usize;
        let count = *self
            .bytes
            .get(style_offset)
            .ok_or_else(|| PreviewFailure::input("The Teningen preview has a bad style offset."))?
            as usize;
        let end = style_offset
            .checked_add(1 + count * STYLE_RECORD_LEN)
            .ok_or_else(|| PreviewFailure::input("The Teningen preview style table overflows."))?;
        let slot = self
            .bytes
            .get_mut(style_offset..end)
            .ok_or_else(|| PreviewFailure::input("The Teningen preview style table is truncated."))?;
        // Only the styles this map carries are stamped. A schema that has grown feature types since
        // the preview map was cut keeps its trailing ones: style ids are assigned in schema document
        // order, so an appended type takes the next free id and leaves every id in here meaning what
        // it meant — and a type the sample has no geometry for cannot change the picture anyway.
        // (`obc-bake`'s `previews.rs` holds the same rule for the published thumbnails.)
        if styles.len() < count {
            return Err(PreviewFailure::input(format!(
                "The preview has {count} styles, but this skin resolves to only {}.",
                styles.len()
            )));
        }
        let stamped = &styles[..count];
        let packed = pack_style_table(stamped);
        if slot.len() != packed.len() {
            return Err(PreviewFailure::input("The Teningen preview style table is not the length it declares."));
        }
        let have_ids: Vec<u8> = slot[1..].chunks_exact(STYLE_RECORD_LEN).map(|record| record[0]).collect();
        let want_ids: Vec<u8> = stamped.iter().map(|style| style.id).collect();
        if have_ids != want_ids {
            return Err(PreviewFailure::input(
                "The preview map belongs to a different schema revision; refresh the builder deployment.",
            ));
        }
        slot.copy_from_slice(&packed);
        self.bytes[HEADER_MARKER_COLOR_AT..HEADER_MARKER_COLOR_AT + 2]
            .copy_from_slice(&skin.marker_color.to_le_bytes());
        self.tables = MapTables::parse(&SliceSource(&self.bytes)).map_err(read_failure)?;
        self.dirty = true;
        Ok(())
    }

    /// Move the rendered map by a logical-frame pixel delta. Invalid deltas are
    /// ignored at this trust boundary; valid moves are clamped to the map bbox.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        if !dx.is_finite() || !dy.is_finite() || (dx == 0.0 && dy == 0.0) {
            return;
        }
        // Pointer capture can report a coordinate far outside the element. One
        // event never needs to move more than a whole frame; bounding it also
        // keeps `Viewport::to_map` away from hostile float-to-i32 saturation.
        let dx = dx.clamp(-(FRAME_W as f32), FRAME_W as f32);
        let dy = dy.clamp(-(FRAME_H as f32), FRAME_H as f32);
        let viewport = self.viewport();
        let (lon, lat) = viewport.to_map(FRAME_W as f32 / 2.0 - dx, FRAME_H as f32 / 2.0 - dy);
        let before = self.camera;
        self.camera.lon = lon;
        self.camera.lat = lat;
        self.clamp_camera();
        self.dirty |= self.camera != before;
    }

    /// Zoom around `(x, y)` in logical-frame pixels. `factor > 1` zooms in.
    /// The point under the cursor stays under it unless a coverage edge clamps
    /// the camera. Non-finite and non-positive input is ignored.
    pub fn zoom_at(&mut self, factor: f32, x: f32, y: f32) {
        if !factor.is_finite() || factor <= 0.0 || !x.is_finite() || !y.is_finite() {
            return;
        }
        let x = x.clamp(0.0, FRAME_W as f32);
        let y = y.clamp(0.0, FRAME_H as f32);
        let anchor = self.viewport().to_map(x, y);
        let before = self.camera;
        self.camera.meters_per_pixel = (self.camera.meters_per_pixel / factor).clamp(self.min_mpp, self.max_mpp);

        let after = self.viewport().to_map(x, y);
        self.camera.lon = add_delta(self.camera.lon, anchor.0 as i64 - after.0 as i64);
        self.camera.lat = add_delta(self.camera.lat, anchor.1 as i64 - after.1 as i64);
        self.clamp_camera();
        self.dirty |= self.camera != before;
    }

    pub fn reset_camera(&mut self) {
        if self.camera != self.default_camera {
            self.camera = self.default_camera;
            self.dirty = true;
        }
    }

    pub fn stats(&self) -> PreviewStats {
        let source = SliceSource(&self.bytes);
        let reader = Reader::new(&source, &self.tables, &self.cache);
        let lod_index = reader.select_lod_for_mpp(self.camera.meters_per_pixel);
        PreviewStats {
            camera_lon: self.camera.lon,
            camera_lat: self.camera.lat,
            meters_per_pixel: self.camera.meters_per_pixel,
            lod_index,
            lod_count: self.tables.lods().len(),
            features_drawn: self.render_stats.features_drawn,
            features_dropped: self.render_stats.features_dropped,
            points_drawn: self.render_stats.points_drawn,
            span_utilization: self.render_stats.span_utilization,
            point_utilization: self.render_stats.point_utilization,
            ring_utilization: self.render_stats.ring_utilization,
        }
    }

    pub fn frame(&mut self) -> &[u8] {
        if self.dirty {
            let source = SliceSource(&self.bytes);
            let reader = Reader::new(&source, &self.tables, &self.cache);
            let background = reader.backdrop_style().map_or(0xFFFF, |style| style.color);
            let viewport = self.viewport();
            self.render_stats =
                self.renderer.render(&mut self.frame, &reader, &viewport, device_color(background), device_color);
            self.dirty = false;
        }
        self.frame.as_rgba()
    }

    fn viewport(&self) -> Viewport {
        Viewport::new(
            FRAME_W as f32,
            FRAME_H as f32,
            self.camera.lon,
            self.camera.lat,
            zoom_for_mpp(self.camera.meters_per_pixel),
        )
    }

    fn clamp_camera(&mut self) {
        // First keep the whole viewport within the real header bbox. Then keep
        // a small view within the dense requested crop; once the viewport is
        // wider than that crop, keep its *centre* in the crop instead. This
        // reaches the complete LOD ladder without turning complete-way header
        // overhang into a sparse pannable region.
        for _ in 0..3 {
            let view = self.viewport().visible_bbox();
            let header = self.tables.bbox;
            self.camera.lon =
                add_delta(self.camera.lon, axis_shift(view.min_lon, view.max_lon, header.min_lon, header.max_lon));
            self.camera.lat =
                add_delta(self.camera.lat, axis_shift(view.min_lat, view.max_lat, header.min_lat, header.max_lat));

            let view = self.viewport().visible_bbox();
            let lon_shift = coverage_shift(
                self.camera.lon,
                view.min_lon,
                view.max_lon,
                self.camera_bounds.min_lon,
                self.camera_bounds.max_lon,
            );
            let lat_shift = coverage_shift(
                self.camera.lat,
                view.min_lat,
                view.max_lat,
                self.camera_bounds.min_lat,
                self.camera_bounds.max_lat,
            );
            self.camera.lon = add_delta(self.camera.lon, lon_shift);
            self.camera.lat = add_delta(self.camera.lat, lat_shift);
        }
    }
}

impl SchemaMapPreview {
    pub fn open(bytes: Vec<u8>) -> Result<Self, PreviewFailure> {
        let tables = MapTables::parse(&SliceSource(&bytes)).map_err(read_failure)?;
        Ok(Self {
            bytes,
            tables,
            cache: MapCache::new_boxed(),
            renderer: Box::new(MapRenderer::new()),
            frame: RgbaFrame::new(SCHEMA_FRAME_W, SCHEMA_FRAME_H),
            meters_per_pixel: DEFAULT_METERS_PER_PIXEL,
            render_stats: RenderStats::default(),
            dirty: true,
        })
    }

    /// Select any authored LOD through the renderer's ordinary m/px dispatch.
    /// The generous upper cap intentionally permits coarsest-LOD inspection
    /// even if a custom/fixture map's bbox is narrower than that whole frame.
    pub fn set_meters_per_pixel(&mut self, value: f32) {
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        let next = value.clamp(MIN_METERS_PER_PIXEL, MAX_SCHEMA_MPP);
        if next != self.meters_per_pixel {
            self.meters_per_pixel = next;
            self.dirty = true;
        }
    }

    pub fn meters_per_pixel(&self) -> f32 {
        self.meters_per_pixel
    }

    pub fn lod_index(&self) -> usize {
        let source = SliceSource(&self.bytes);
        Reader::new(&source, &self.tables, &self.cache).select_lod_for_mpp(self.meters_per_pixel)
    }

    pub fn lod_count(&self) -> usize {
        self.tables.lods().len()
    }

    pub fn stats(&self) -> RenderStats {
        self.render_stats
    }

    pub fn frame(&mut self) -> &[u8] {
        if self.dirty {
            let source = SliceSource(&self.bytes);
            let reader = Reader::new(&source, &self.tables, &self.cache);
            let background = reader.backdrop_style().map_or(0xFFFF, |style| style.color);
            let bbox = self.tables.bbox;
            let center_lon = ((bbox.min_lon as i64 + bbox.max_lon as i64) / 2) as i32;
            let center_lat = ((bbox.min_lat as i64 + bbox.max_lat as i64) / 2) as i32;
            let camera_lon =
                if bbox.min_lon <= CAMERA_LON && CAMERA_LON <= bbox.max_lon { CAMERA_LON } else { center_lon };
            let camera_lat =
                if bbox.min_lat <= CAMERA_LAT && CAMERA_LAT <= bbox.max_lat { CAMERA_LAT } else { center_lat };
            let viewport = Viewport::new(
                SCHEMA_FRAME_W as f32,
                SCHEMA_FRAME_H as f32,
                camera_lon,
                camera_lat,
                zoom_for_mpp(self.meters_per_pixel),
            );
            self.render_stats =
                self.renderer.render(&mut self.frame, &reader, &viewport, device_color(background), device_color);
            self.dirty = false;
        }
        self.frame.as_rgba()
    }
}

fn axis_shift(view_min: i32, view_max: i32, map_min: i32, map_max: i32) -> i64 {
    let view_span = view_max as i64 - view_min as i64;
    let map_span = map_max as i64 - map_min as i64;
    if view_span >= map_span {
        return (map_min as i64 + map_max as i64) / 2 - (view_min as i64 + view_max as i64) / 2;
    }
    if view_min < map_min {
        map_min as i64 - view_min as i64
    } else if view_max > map_max {
        map_max as i64 - view_max as i64
    } else {
        0
    }
}

fn coverage_shift(camera: i32, view_min: i32, view_max: i32, dense_min: i32, dense_max: i32) -> i64 {
    let view_span = view_max as i64 - view_min as i64;
    let dense_span = dense_max as i64 - dense_min as i64;
    if view_span > dense_span {
        camera.clamp(dense_min, dense_max) as i64 - camera as i64
    } else {
        axis_shift(view_min, view_max, dense_min, dense_max)
    }
}

fn contains_bbox(outer: BBox, inner: BBox) -> bool {
    outer.min_lon <= inner.min_lon
        && outer.min_lat <= inner.min_lat
        && outer.max_lon >= inner.max_lon
        && outer.max_lat >= inner.max_lat
}

fn add_delta(value: i32, delta: i64) -> i32 {
    (value as i64 + delta).clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

fn maximum_fitting_mpp(bbox: BBox, camera_bounds: BBox) -> f32 {
    let lon =
        (((bbox.min_lon as i64 + bbox.max_lon as i64) / 2) as i32).clamp(camera_bounds.min_lon, camera_bounds.max_lon);
    let lat =
        (((bbox.min_lat as i64 + bbox.max_lat as i64) / 2) as i32).clamp(camera_bounds.min_lat, camera_bounds.max_lat);
    let fits = |mpp| {
        let visible = Viewport::new(FRAME_W as f32, FRAME_H as f32, lon, lat, zoom_for_mpp(mpp)).visible_bbox();
        visible.min_lon >= bbox.min_lon
            && visible.max_lon <= bbox.max_lon
            && visible.min_lat >= bbox.min_lat
            && visible.max_lat <= bbox.max_lat
    };

    let mut low = f32::EPSILON;
    let mut high = MIN_METERS_PER_PIXEL;
    while high < MAX_MPP_SEARCH && fits(high) {
        low = high;
        high *= 2.0;
    }
    high = high.min(MAX_MPP_SEARCH);
    for _ in 0..32 {
        let middle = (low + high) / 2.0;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low.max(f32::EPSILON)
}

fn device_color(color: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_device64(color);
    Rgb888::new(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    use obc_pack::catalog::feature_type_ids;
    use obc_pack::config::{Config, LineStyle};
    use obc_pack::grid::BandTable;

    const MAP: &[u8] = include_bytes!("../../../host/obc-bake/assets/teningen-preview.obcm");
    const SCHEMA_CONFIG: &str = include_str!("../../../builder/presets/schema.json");

    fn schema_json() -> String {
        let config = Config::parse(SCHEMA_CONFIG).expect("schema config parses");
        let bands = BandTable::recommended().bands;
        let mut styles = feature_type_ids(&config)
            .into_iter()
            .map(|(feature_type, id)| serde_json::json!({ "id": id, "feature_type": feature_type }))
            .collect::<Vec<_>>();
        styles.sort_by_key(|style| style["id"].as_u64().unwrap());
        serde_json::json!({
            "id": "bikepacking",
            "revision": 1,
            "obcm_version": obc_formats::obcm::VERSION,
            "lods": config.lods.iter().enumerate().map(|(index, lod)| serde_json::json!({
                "index": index,
                "max_mpp": lod.max_mpp,
                "band": bands.iter().find(|band| band.lods.contains(&index)).unwrap().id,
            })).collect::<Vec<_>>(),
            "bands": bands,
            "styles": styles,
            "routing": {
                "min_component_edges": config.routing.min_component_edges,
                "profiles": config.routing.profiles.iter().map(|profile| &profile.name).collect::<Vec<_>>(),
            },
            "chunk_size": config.chunk_size,
        })
        .to_string()
    }

    fn skin_json(id: &str) -> String {
        let text = match id {
            "default" => include_str!("../../../builder/presets/skins/default.json"),
            "dusk" => include_str!("../../../builder/presets/skins/dusk.json"),
            _ => panic!("unknown shipped skin"),
        };
        let config = Config::parse(text).expect("skin config parses");
        let ids = feature_type_ids(&config);
        let mut styles = config
            .features
            .iter()
            .flat_map(|(key, values)| {
                values.iter().map(move |(value, style)| {
                    serde_json::json!({
                        "feature_type": format!("{key}.{value}"),
                        "color": style.color,
                        "weight": style.weight,
                        "z_index": style.z_index,
                        "priority": style.priority,
                        "dashed": style.line_style == LineStyle::Dashed,
                        "color2": style.color2,
                    })
                })
            })
            .collect::<Vec<_>>();
        styles.sort_by_key(|style| ids[style["feature_type"].as_str().unwrap()]);
        serde_json::json!({
            "id": id,
            "name": id,
            "marker_color": config.marker_color,
            "styles": styles,
        })
        .to_string()
    }

    #[test]
    fn restyles_one_resident_map_through_the_device_renderer() {
        let schema = schema_json();
        let day_skin = skin_json("default");
        let dusk_skin = skin_json("dusk");
        let mut preview = MapPreview::open(MAP.to_vec(), &schema, &day_skin).expect("default opens");
        let day = preview.frame().to_vec();
        assert_eq!(day.len(), (FRAME_W * FRAME_H * 4) as usize);
        assert!(day.chunks_exact(4).all(|px| px[3] == 0xFF));

        preview.set_skin(&dusk_skin).expect("dusk restamps");
        let dusk = preview.frame().to_vec();
        assert_ne!(day, dusk, "a skin edit must change the rendered scene");
    }

    #[test]
    fn refuses_schema_space_edits() {
        let schema = schema_json();
        let day_skin = skin_json("default");
        let mut preview = MapPreview::open(MAP.to_vec(), &schema, &day_skin).expect("default opens");
        let mut skin: serde_json::Value = serde_json::from_str(&day_skin).unwrap();
        skin["styles"].as_array_mut().unwrap().remove(0);
        let err = preview.set_skin(&skin.to_string()).expect_err("missing schema type is refused");
        assert_eq!(err.code, PreviewErrorCode::Input);
    }

    fn opened() -> MapPreview {
        MapPreview::open(MAP.to_vec(), &schema_json(), &skin_json("default")).expect("preview opens")
    }

    #[test]
    fn reports_the_production_lod_at_every_exact_threshold() {
        let mut preview = opened();
        let thresholds = [(31.0, 0), (30.0, 1), (16.0, 2), (10.0, 3), (5.0, 4), (3.0, 5), (1.2, 6), (0.5, 6)];
        assert_eq!(preview.tables.lods().len(), 7, "fixture must exercise the complete shipped ladder");
        for (mpp, expected) in thresholds {
            preview.camera.meters_per_pixel = mpp;
            assert_eq!(preview.stats().lod_index, expected, "wrong LOD at {mpp} m/px");
        }
        preview.camera.meters_per_pixel = f32::from_bits(30.0_f32.to_bits() + 1);
        assert_eq!(preview.stats().lod_index, 0, "one float above the 30 m/px ceiling must fall back coarse");
    }

    #[test]
    fn wheel_scale_can_reach_every_lod_but_never_expose_blank_coverage() {
        let mut preview = opened();
        preview.zoom_at(1.0e-6, FRAME_W as f32 / 2.0, FRAME_H as f32 / 2.0);
        assert!(preview.stats().meters_per_pixel > 30.0, "fixture is already wide enough for the coarsest LOD");
        assert_eq!(preview.stats().lod_index, 0);
        assert_view_inside_map(&preview);

        preview.zoom_at(1.0e6, FRAME_W as f32 / 2.0, FRAME_H as f32 / 2.0);
        assert_eq!(preview.stats().meters_per_pixel, MIN_METERS_PER_PIXEL);
        assert_eq!(preview.stats().lod_index, 6);
        assert_view_inside_map(&preview);
    }

    #[test]
    fn pan_and_cursor_anchored_zoom_clamp_all_edges() {
        let mut preview = opened();
        let anchor_xy = (37.0, 191.0);
        let anchor_before = preview.viewport().to_map(anchor_xy.0, anchor_xy.1);
        preview.zoom_at(2.0, anchor_xy.0, anchor_xy.1);
        let anchor_after = preview.viewport().to_map(anchor_xy.0, anchor_xy.1);
        assert!((anchor_before.0 - anchor_after.0).abs() <= 1);
        assert!((anchor_before.1 - anchor_after.1).abs() <= 1);

        for (dx, dy) in [(1.0e9, 0.0), (-1.0e9, 0.0), (0.0, 1.0e9), (0.0, -1.0e9)] {
            preview.pan_by(dx, dy);
            assert_view_inside_map(&preview);
        }
    }

    #[test]
    fn edge_clamps_use_the_requested_dense_crop_and_keep_a_meaningful_scene() {
        let mut preview = opened();
        assert_eq!(preview.camera_bounds, TENINGEN_COVERAGE);
        for (dx, dy) in [(1.0e9, 1.0e9), (-1.0e9, 1.0e9), (1.0e9, -1.0e9), (-1.0e9, -1.0e9)] {
            preview.reset_camera();
            preview.pan_by(dx, dy);
            assert_view_inside_bounds(&preview, TENINGEN_COVERAGE);
            let pixels = preview.frame().to_vec();
            let stats = preview.stats();
            let colored = non_modal_pixels(&pixels);
            assert!(stats.features_drawn >= 20, "edge view is too sparse to be a useful preview: {stats:?}");
            assert!(colored >= 1_000, "edge view is effectively blank: only {colored} non-modal pixels");
        }

        preview.reset_camera();
        preview.zoom_at(1.0e-6, 120.0, 120.0);
        let pixels = preview.frame().to_vec();
        let stats = preview.stats();
        let colored = non_modal_pixels(&pixels);
        assert_eq!(stats.lod_index, 0);
        assert!(stats.features_drawn >= 3, "coarsest rung must retain useful Teningen context: {stats:?}");
        assert!(colored >= 1_000, "coarsest rung is effectively blank: only {colored} non-modal pixels");
    }

    #[test]
    fn invalid_camera_input_is_a_noop_and_reset_is_exact() {
        let mut preview = opened();
        let original = preview.camera;
        preview.pan_by(f32::NAN, 4.0);
        preview.pan_by(4.0, f32::INFINITY);
        preview.zoom_at(0.0, 1.0, 1.0);
        preview.zoom_at(f32::NAN, 1.0, 1.0);
        preview.zoom_at(2.0, f32::INFINITY, 1.0);
        assert_eq!(preview.camera, original);

        preview.pan_by(20.0, -10.0);
        preview.zoom_at(1.4, 120.0, 120.0);
        assert_ne!(preview.camera, original);
        preview.reset_camera();
        assert_eq!(preview.camera, original);
    }

    #[test]
    fn skin_restamping_keeps_the_interactive_camera_and_real_stats() {
        let mut preview = opened();
        preview.pan_by(23.0, -17.0);
        preview.zoom_at(1.7, 40.0, 80.0);
        let camera = preview.camera;
        let day = preview.frame().to_vec();
        let day_stats = preview.stats();
        assert_eq!(day_stats.lod_index, preview.render_stats.lod);
        assert!(day_stats.features_drawn > 0);

        preview.set_skin(&skin_json("dusk")).expect("dusk restamps");
        let dusk = preview.frame().to_vec();
        assert_eq!(preview.camera, camera, "presentation changes must not reset the user's view");
        assert_eq!(preview.stats().lod_index, day_stats.lod_index);
        assert_ne!(day, dusk);
    }

    fn assert_view_inside_map(preview: &MapPreview) {
        let view = preview.viewport().visible_bbox();
        let map = preview.tables.bbox;
        assert!(view.min_lon >= map.min_lon, "west edge escaped: {view:?} vs {map:?}");
        assert!(view.max_lon <= map.max_lon, "east edge escaped: {view:?} vs {map:?}");
        assert!(view.min_lat >= map.min_lat, "south edge escaped: {view:?} vs {map:?}");
        assert!(view.max_lat <= map.max_lat, "north edge escaped: {view:?} vs {map:?}");
    }

    fn assert_view_inside_bounds(preview: &MapPreview, bounds: BBox) {
        let view = preview.viewport().visible_bbox();
        assert!(view.min_lon >= bounds.min_lon, "west edge escaped dense crop: {view:?}");
        assert!(view.max_lon <= bounds.max_lon, "east edge escaped dense crop: {view:?}");
        assert!(view.min_lat >= bounds.min_lat, "south edge escaped dense crop: {view:?}");
        assert!(view.max_lat <= bounds.max_lat, "north edge escaped dense crop: {view:?}");
    }

    fn non_modal_pixels(rgba: &[u8]) -> usize {
        let mut counts = std::collections::HashMap::<[u8; 3], usize>::new();
        for pixel in rgba.chunks_exact(4) {
            *counts.entry([pixel[0], pixel[1], pixel[2]]).or_default() += 1;
        }
        let modal = counts.values().copied().max().unwrap_or(0);
        (FRAME_W * FRAME_H) as usize - modal
    }

    #[test]
    fn raw_schema_map_uses_device_geometry_lod_dispatch_and_frame_budgets() {
        let mut preview = SchemaMapPreview::open(MAP.to_vec()).expect("packed map opens without restamping");
        assert_eq!((SCHEMA_FRAME_W, SCHEMA_FRAME_H), (240, 320));

        // These are representative m/px values for the fixture's seven authored
        // ranges. Selection is the production Reader policy, not a UI formula.
        for (mpp, expected) in [(40.0, 0), (30.0, 1), (16.0, 2), (10.0, 3), (5.0, 4), (3.0, 5), (1.2, 6)] {
            preview.set_meters_per_pixel(mpp);
            assert_eq!(preview.lod_index(), expected, "{mpp} m/px");
        }

        preview.set_meters_per_pixel(5.0);
        let pixels = preview.frame();
        assert_eq!(pixels.len(), (240 * 320 * 4) as usize);
        assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 0xff));
        let stats = preview.stats();
        assert_eq!(stats.lod, 4);
        assert!(stats.features_drawn <= obc_render::MAX_SPANS);
        assert!(stats.points_drawn <= obc_render::MAX_FRAME_POINTS);
        assert!(stats.line_rings + stats.poly_rings <= obc_render::MAX_FRAME_RINGS);
    }
}
