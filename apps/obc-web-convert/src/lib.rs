//! `obc-web-convert` — the hosted builder's conversion bridge (epic #894, A2).
//!
//! The hosted tier has no backend, and this is what removes the last reason it would need one:
//! `gpx_to_obcr` and `track_to_gpx` are `no_std` Rust in crates that already compile to wasm, so
//! route conversion runs in the visitor's browser through **the exact code the device and the CLI
//! run**. Not a re-implementation in TypeScript — the same bytes, by construction.
//!
//! Deliberately not a framework host like [`obc-web-demo`](../obc_web_demo/index.html): no frame
//! loop, no canvas, no state. Two pure functions and an error vocabulary.
//!
//! | export | contract |
//! | :-- | :-- |
//! | `obc_convert_gpx_to_obcr(bytes, name) -> Uint8Array` | a GPX file's bytes → a `.obcr` route |
//! | `obc_convert_track_to_gpx(bytes, name) -> string` | a recorded `.obct` log → a GPX 1.1 document |
//! | `obc_convert_obcr_to_track(bytes) -> Float64Array` | a `.obcr` route → flat `[lat°, lon°, ele m]` triples |
//! | `obc_convert_obcr_to_waypoints(bytes) -> Array` | a `.obcr` route's waypoint table → `{name, lat, lon, ele, category, distAlongM}` objects |
//!
//! A failure crosses to JS as a thrown `Error` whose `message` is written for a rider and whose
//! `code` ([`ErrorCode`]) is the stable identifier a caller branches on. Every
//! [`obc_formats::io::Error`] variant is mapped by hand in [`convert`] — the matches are
//! exhaustive so a new variant breaks this build instead of quietly inheriting someone else's
//! wording.
//!
//! The conversion core ([`convert`]) is target-independent and unit-tested natively by the
//! workspace `cargo test`; only the bindgen shim below is wasm-specific.

mod convert;

pub use convert::{
    gpx_to_obcr, obcr_to_track, obcr_to_waypoints, track_to_gpx, ConvertFailure, ErrorCode, RouteWaypoint,
    MAX_STORED_POINTS,
};

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    use crate::convert::ConvertFailure;

    /// Module start (wasm-bindgen runs this during instantiation): surface Rust panics in the
    /// console instead of an opaque `unreachable` trap. Nothing else — there is no state to build,
    /// so instantiation stays as cheap as the download.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
    }

    /// Convert a GPX file's bytes into `.obcr` bytes, naming the route `name` (the OBCR header
    /// truncates it to the format's cap on a char boundary — an over-long name is not an error).
    ///
    /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_gpx_to_obcr(bytes: &[u8], name: &str) -> Result<Vec<u8>, JsValue> {
        crate::convert::gpx_to_obcr(bytes, name).map_err(to_js)
    }

    /// Convert a recorded `.obct` ride log into a GPX 1.1 document, naming the track `name`.
    ///
    /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_track_to_gpx(bytes: &[u8], name: &str) -> Result<String, JsValue> {
        crate::convert::track_to_gpx(bytes, name).map_err(to_js)
    }

    /// Decode a `.obcr` route's polyline for the device page's preview: flat `[lat°, lon°, ele m]`
    /// triples in route order, crossing as one `Float64Array`.
    ///
    /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_obcr_to_track(bytes: &[u8]) -> Result<Vec<f64>, JsValue> {
        crate::convert::obcr_to_track(bytes).map_err(to_js)
    }

    /// Decode a `.obcr` route's waypoint table (OBCR spec §4): an `Array` of plain
    /// `{name, lat, lon, ele, category, distAlongM}` objects in route order (ascending
    /// `distAlongM`). `ele` is `null` where the source carried none; `category` is the stored
    /// byte raw (`0` generic, `1..=6` the OBCM §7.4 ids — render anything else as generic);
    /// `distAlongM` is the **stored** placement-time distance in meters, not a recomputation
    /// (see [`crate::convert::obcr_to_waypoints`]). A route without waypoints yields `[]`.
    ///
    /// Plain objects rather than a flat array because names are strings: ≤ 32 waypoints cross per
    /// route (the converter's cap), so per-entry objects cost nothing that matters.
    ///
    /// Throws an `Error` carrying `code` + `message` on failure; see [`crate::ErrorCode`].
    #[wasm_bindgen]
    pub fn obc_convert_obcr_to_waypoints(bytes: &[u8]) -> Result<js_sys::Array, JsValue> {
        let wps = crate::convert::obcr_to_waypoints(bytes).map_err(to_js)?;
        let arr = js_sys::Array::new();
        for w in wps {
            let obj = js_sys::Object::new();
            set(&obj, "name", &JsValue::from_str(&w.name));
            set(&obj, "lat", &JsValue::from_f64(w.lat));
            set(&obj, "lon", &JsValue::from_f64(w.lon));
            set(&obj, "ele", &w.ele.map_or(JsValue::NULL, JsValue::from_f64));
            set(&obj, "category", &JsValue::from_f64(f64::from(w.category)));
            set(&obj, "distAlongM", &JsValue::from_f64(f64::from(w.dist_along_m)));
            arr.push(&obj.into());
        }
        Ok(arr)
    }

    /// `Reflect::set` on a fresh plain object — which cannot fail (only frozen/exotic targets
    /// can), so the result is ignored for the same reason it is in [`to_js`].
    fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
        let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
    }

    /// Build the JS exception: a real `Error` instance (so it carries a stack and survives
    /// `instanceof Error`), renamed, with the stable code hung off it as a plain property.
    ///
    /// A `#[wasm_bindgen]` struct would also cross the boundary, but it would not *be* an `Error`
    /// — `catch (e) { e.message }` and every logger that formats errors would come up empty.
    fn to_js(f: ConvertFailure) -> JsValue {
        let err = js_sys::Error::new(&f.message);
        err.set_name("ObcConvertError");
        // `Reflect::set` only fails on a frozen/exotic target; `err` is a fresh object, so this
        // cannot. Ignored rather than unwrapped so a surprise here still throws a usable Error
        // (with a message) instead of trapping the module.
        let _ = js_sys::Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(f.code.as_str()));
        err.into()
    }
}
