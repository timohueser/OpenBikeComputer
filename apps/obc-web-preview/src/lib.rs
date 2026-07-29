//! `obc-web-preview` — the builder's preset-preview renderer (epic #894, B2 — issue #899).
//!
//! The hosted tier has no style editor: presets are the only styling a visitor ever sees, so each
//! one has to show what it actually draws. This is what makes that possible without a server —
//! the same `obc-reader` + `obc-render` code the nRF54L runs, compiled to wasm, handed a small
//! demo `.obcm` baked from that preset's own config. Not a mockup and not a screenshot pipeline
//! anyone has to remember to re-run: the browser renders the map, at the panel's own 240×320,
//! from bytes the packer produced.
//!
//! Every preset's demo map is baked from **the same source extract and the same bbox**
//! (`builder/bake-previews.sh`), so the cards compare like-for-like. The differences between them
//! are the presets, and only the presets.
//!
//! | export | contract |
//! | :-- | :-- |
//! | `new MapPreview(bytes)` | open a demo `.obcm`; throws an `Error` carrying `code` |
//! | `preview.frame()` | RGBA view of the 240×320 frame, for `putImageData` |
//! | `preview.pan(dx, dy)` | drag the map by a screen-space delta in pixels |
//! | `preview.zoom_by(factor)` | scale the zoom, clamped to the preview's limits |
//! | `preview.reset()` | back to the opening fit-the-map view |
//! | `preview.is_dirty()` | whether the next `frame()` will redraw |
//! | `preview.meters_per_pixel()` | the current ground scale |
//! | `preview.free()` | release the ≈370 KB of reader cache + render scratch |
//!
//! The preview core ([`preview`]) is target-independent and unit-tested natively by the workspace
//! `cargo test`; only the bindgen shim below is wasm-specific.

mod preview;

pub use preview::{MapPreview, PreviewErrorCode, PreviewFailure, FRAME_H, FRAME_W};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::preview::{MapPreview, PreviewFailure};

    /// Module start (wasm-bindgen runs this during instantiation): surface Rust panics in the
    /// console instead of an opaque `unreachable` trap.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    /// One open demo map with a camera over it.
    ///
    /// A page holds one per card. It owns real memory — the map bytes, a ≈277 KB chunk cache and
    /// ≈90 KB of render scratch — so a card that leaves the viewport should `free()` it rather
    /// than wait for a GC that wasm-bindgen does not provide.
    #[wasm_bindgen(js_name = MapPreview)]
    pub struct JsMapPreview(MapPreview);

    #[wasm_bindgen(js_class = MapPreview)]
    impl JsMapPreview {
        /// Open a demo `.obcm`, parked on the fit-the-whole-map view.
        ///
        /// Throws an `Error` carrying `code` + `message`; see [`crate::PreviewErrorCode`].
        #[wasm_bindgen(constructor)]
        pub fn new(bytes: Vec<u8>) -> Result<JsMapPreview, JsValue> {
            MapPreview::open(bytes).map(JsMapPreview).map_err(to_js)
        }

        /// The panel width these frames are, in pixels.
        #[wasm_bindgen(getter)]
        pub fn width(&self) -> u32 {
            crate::FRAME_W
        }

        /// The panel height these frames are, in pixels.
        #[wasm_bindgen(getter)]
        pub fn height(&self) -> u32 {
            crate::FRAME_H
        }

        /// A **view** of the current RGBA frame over wasm memory — wrap it in an `ImageData` and
        /// `putImageData` it immediately. Do not retain it: any later wasm call may grow the
        /// memory and detach the view. Zero-copy on the Rust side.
        ///
        /// Built through the explicit `new Uint8ClampedArray(memory.buffer, ptr, len)`
        /// constructor, for the reason `obc-web-demo` documents: `Uint8ClampedArray::view` hands
        /// back a plain `Uint8Array` at runtime, and the `ImageData` constructor type-checks for
        /// the *clamped* array.
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

        /// Drag the map by a screen-space delta in pixels — `pan(10, 0)` moves the content right,
        /// which is what a pointer drag means. Clamped to the map's own bbox.
        pub fn pan(&mut self, dx: f32, dy: f32) {
            self.0.pan(dx, dy);
        }

        /// Scale the zoom by `factor` (`>1` zooms in), clamped to the preview's limits.
        pub fn zoom_by(&mut self, factor: f32) {
            self.0.zoom_by(factor);
        }

        /// Re-aim the opening view at an explicit bbox in microdegrees, and go there. The site
        /// passes the one bbox every demo map was cut from, so every card frames the same ground
        /// — see [`MapPreview::fit_bbox`](crate::MapPreview::fit_bbox) for why the map's own
        /// header bbox will not do.
        pub fn fit_bbox(&mut self, min_lon: i32, min_lat: i32, max_lon: i32, max_lat: i32) {
            self.0.fit_bbox(min_lon, min_lat, max_lon, max_lat);
        }

        /// Back to the opening view.
        pub fn reset(&mut self) {
            self.0.reset();
        }

        /// Whether the next [`frame`](JsMapPreview::frame) call will redraw. A page can skip the
        /// `putImageData` when nothing moved.
        pub fn is_dirty(&self) -> bool {
            self.0.is_dirty()
        }

        /// Ground metres per pixel at the current zoom.
        pub fn meters_per_pixel(&self) -> f32 {
            self.0.meters_per_pixel()
        }
    }

    /// A failure as a real JS `Error` carrying a stable `code` property, so a caller branches on
    /// the cause rather than on message text — the same shape the conversion bridge throws.
    fn to_js(failure: PreviewFailure) -> JsValue {
        let err = js_sys::Error::new(&failure.message);
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(failure.code.as_str()));
        err.into()
    }
}
