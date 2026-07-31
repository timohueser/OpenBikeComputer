//! `obc-web-preview` — the product skin editor's live, device-honest preview.
//!
//! The browser hands this bridge the bakery's canonical Teningen OBCM plus the
//! catalog schema and a skin. The bridge resolves the skin with the same
//! `obcm-assemble` code used for a real map, replaces only the style table and
//! marker color, then draws the fixed 240×240, 5 m/px scene through the same
//! `obc-reader` + `obc-render` path as the nRF54L firmware. No geometry or LOD
//! setting is editable here: this is intentionally a skin-space surface.

mod preview;

pub use preview::{MapPreview, PreviewErrorCode, PreviewFailure, FRAME_H, FRAME_W};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::preview::{MapPreview, PreviewFailure};

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
    }

    fn to_js(failure: PreviewFailure) -> JsValue {
        let err = js_sys::Error::new(&failure.message);
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(failure.code.as_str()));
        err.into()
    }
}
