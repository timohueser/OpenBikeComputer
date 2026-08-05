//! `obc-skin-preview` — the product skin editor's live, device-honest preview.
//!
//! The browser hands this bridge the bakery's canonical Teningen OBCM plus the
//! catalog schema and a skin. The bridge resolves the skin with the same
//! `obcm-assemble` code used for a real map, replaces only the style table and
//! marker color, then draws a 240×240 scene through the same `obc-reader` +
//! `obc-render` path as the nRF54L firmware. Camera changes stay in this bridge:
//! browser callers ask to pan or zoom in screen pixels and receive the actual
//! renderer LOD and frame-budget statistics rather than duplicating projection
//! or LOD-selection policy in TypeScript. No geometry or LOD setting is editable
//! here: this is intentionally a skin-space surface.

mod preview;

pub use preview::{
    MapPreview, PreviewErrorCode, PreviewFailure, PreviewStats, SchemaMapPreview, FRAME_H, FRAME_W, SCHEMA_FRAME_H,
    SCHEMA_FRAME_W,
};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::preview::{MapPreview, PreviewFailure, SchemaMapPreview};

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    #[wasm_bindgen(js_name = SkinPreview)]
    pub struct JsSkinPreview(MapPreview);

    #[wasm_bindgen(js_class = SkinPreview)]
    impl JsSkinPreview {
        /// Open the canonical map and apply the initial skin.
        #[wasm_bindgen(constructor)]
        pub fn new(map: Vec<u8>, schema_json: &str, skin_json: &str) -> Result<JsSkinPreview, JsValue> {
            MapPreview::open(map, schema_json, skin_json).map(JsSkinPreview).map_err(to_js)
        }

        #[wasm_bindgen(getter)]
        pub fn width(&self) -> u32 {
            crate::FRAME_W
        }

        #[wasm_bindgen(getter)]
        pub fn height(&self) -> u32 {
            crate::FRAME_H
        }

        /// Restamp presentation bytes only. Geometry stays resident and is not
        /// decoded or assembled again.
        pub fn set_skin(&mut self, skin_json: &str) -> Result<(), JsValue> {
            self.0.set_skin(skin_json).map_err(to_js)
        }

        /// Move the map by a screen-space drag delta. Positive x/y moves the
        /// rendered map right/down, like a physical sheet under the pointer.
        pub fn pan_by(&mut self, dx: f32, dy: f32) {
            self.0.pan_by(dx, dy);
        }

        /// Zoom around one logical-frame point. A factor above one zooms in.
        pub fn zoom_at(&mut self, factor: f32, x: f32, y: f32) {
            self.0.zoom_at(factor, x, y);
        }

        pub fn reset_camera(&mut self) {
            self.0.reset_camera();
        }

        /// A transient RGBA view over wasm memory. The caller copies it into
        /// `ImageData` before making another wasm call.
        pub fn frame(&mut self) -> js_sys::Uint8ClampedArray {
            use wasm_bindgen::JsCast as _;
            let buf = self.0.frame();
            let mem = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>();
            js_sys::Uint8ClampedArray::new_with_byte_offset_and_length(
                &mem.buffer(),
                buf.as_ptr() as u32,
                buf.len() as u32,
            )
        }

        #[wasm_bindgen(getter)]
        pub fn meters_per_pixel(&self) -> f32 {
            self.0.stats().meters_per_pixel
        }

        #[wasm_bindgen(getter)]
        pub fn lod_index(&self) -> u32 {
            self.0.stats().lod_index as u32
        }

        #[wasm_bindgen(getter)]
        pub fn lod_count(&self) -> u32 {
            self.0.stats().lod_count as u32
        }

        #[wasm_bindgen(getter)]
        pub fn features_drawn(&self) -> u32 {
            self.0.stats().features_drawn as u32
        }

        #[wasm_bindgen(getter)]
        pub fn features_dropped(&self) -> u32 {
            self.0.stats().features_dropped as u32
        }

        #[wasm_bindgen(getter)]
        pub fn points_drawn(&self) -> u32 {
            self.0.stats().points_drawn as u32
        }

        #[wasm_bindgen(getter)]
        pub fn span_utilization(&self) -> f32 {
            self.0.stats().span_utilization
        }

        #[wasm_bindgen(getter)]
        pub fn point_utilization(&self) -> f32 {
            self.0.stats().point_utilization
        }

        #[wasm_bindgen(getter)]
        pub fn ring_utilization(&self) -> f32 {
            self.0.stats().ring_utilization
        }
    }

    /// Raw native-packed map for the localhost maintainer schema lab. It has no
    /// skin mutation API: every edit must pass through obc-pack first.
    #[wasm_bindgen(js_name = SchemaPreview)]
    pub struct JsSchemaPreview(SchemaMapPreview);

    #[wasm_bindgen(js_class = SchemaPreview)]
    impl JsSchemaPreview {
        #[wasm_bindgen(constructor)]
        pub fn new(map: Vec<u8>) -> Result<JsSchemaPreview, JsValue> {
            SchemaMapPreview::open(map).map(JsSchemaPreview).map_err(to_js)
        }

        #[wasm_bindgen(getter)]
        pub fn width(&self) -> u32 {
            crate::SCHEMA_FRAME_W
        }

        #[wasm_bindgen(getter)]
        pub fn height(&self) -> u32 {
            crate::SCHEMA_FRAME_H
        }

        pub fn set_meters_per_pixel(&mut self, value: f32) {
            self.0.set_meters_per_pixel(value);
        }

        pub fn frame(&mut self) -> js_sys::Uint8ClampedArray {
            use wasm_bindgen::JsCast as _;
            let buf = self.0.frame();
            let mem = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>();
            js_sys::Uint8ClampedArray::new_with_byte_offset_and_length(
                &mem.buffer(),
                buf.as_ptr() as u32,
                buf.len() as u32,
            )
        }

        #[wasm_bindgen(getter)]
        pub fn meters_per_pixel(&self) -> f32 {
            self.0.meters_per_pixel()
        }

        #[wasm_bindgen(getter)]
        pub fn lod_index(&self) -> u32 {
            self.0.lod_index() as u32
        }

        #[wasm_bindgen(getter)]
        pub fn lod_count(&self) -> u32 {
            self.0.lod_count() as u32
        }

        #[wasm_bindgen(getter)]
        pub fn chunks_visited(&self) -> u32 {
            self.0.stats().chunks_visited as u32
        }

        #[wasm_bindgen(getter)]
        pub fn features_tried(&self) -> u32 {
            self.0.stats().features_tried as u32
        }

        #[wasm_bindgen(getter)]
        pub fn features_drawn(&self) -> u32 {
            self.0.stats().features_drawn as u32
        }

        #[wasm_bindgen(getter)]
        pub fn features_dropped(&self) -> u32 {
            self.0.stats().features_dropped as u32
        }

        #[wasm_bindgen(getter)]
        pub fn points_tried(&self) -> u32 {
            self.0.stats().points_tried as u32
        }

        #[wasm_bindgen(getter)]
        pub fn points_drawn(&self) -> u32 {
            self.0.stats().points_drawn as u32
        }

        #[wasm_bindgen(getter)]
        pub fn spans_used(&self) -> u32 {
            let stats = self.0.stats();
            (stats.line_spans + stats.poly_spans) as u32
        }

        #[wasm_bindgen(getter)]
        pub fn rings_used(&self) -> u32 {
            let stats = self.0.stats();
            (stats.line_rings + stats.poly_rings) as u32
        }

        #[wasm_bindgen(getter)]
        pub fn feature_decode_capacity_drops(&self) -> u32 {
            self.0.stats().feature_decode_capacity_drops
        }

        #[wasm_bindgen(getter)]
        pub fn malformed_features(&self) -> u32 {
            self.0.stats().malformed_features
        }

        #[wasm_bindgen(getter)]
        pub fn map_errors(&self) -> u32 {
            let stats = self.0.stats();
            stats.map_structure_failures + stats.map_read_failures + stats.map_cache_contentions
        }

        #[wasm_bindgen(getter)]
        pub fn max_feature_points(&self) -> u32 {
            obc_reader::MAX_FEAT_PTS as u32
        }

        #[wasm_bindgen(getter)]
        pub fn max_feature_rings(&self) -> u32 {
            obc_reader::MAX_FEAT_RINGS as u32
        }

        #[wasm_bindgen(getter)]
        pub fn max_spans(&self) -> u32 {
            obc_render::MAX_SPANS as u32
        }

        #[wasm_bindgen(getter)]
        pub fn max_frame_points(&self) -> u32 {
            obc_render::MAX_FRAME_POINTS as u32
        }

        #[wasm_bindgen(getter)]
        pub fn max_frame_rings(&self) -> u32 {
            obc_render::MAX_FRAME_RINGS as u32
        }
    }

    /// Build the JS exception: a real `Error` instance (so it carries a stack and survives
    /// `instanceof Error`), renamed, with the stable code hung off it as a plain property — the
    /// same shape `obc-web-convert` and `obc-web-assemble` throw.
    fn to_js(failure: PreviewFailure) -> JsValue {
        let err = js_sys::Error::new(&failure.message);
        err.set_name("ObcSkinPreviewError");
        // `Reflect::set` only fails on a frozen/exotic target; `err` is a fresh object, so this
        // cannot. Ignored rather than unwrapped so a surprise here still throws a usable Error
        // (with a message) instead of trapping the module.
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(failure.code.as_str()));
        err.into()
    }
}
